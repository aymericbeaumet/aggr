//! Bounded, lossless local copies of article-body images.
//!
//! Original bytes remain the master. Responsive renditions are resized once from the oriented
//! decode and encoded as lossless WebP, then decoded again to prove pixel equality before use.

pub mod placeholder;
pub(crate) mod srcset;
mod vector;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use image::{
    AnimationDecoder as _, DynamicImage, ExtendedColorType, GenericImageView as _,
    ImageDecoder as _, ImageFormat, ImageReader,
};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::sync::Semaphore;
use url::Url;

use crate::config::{FetchConfig, Source};
use crate::http;
use crate::model::{ArticleImage, ImageFile};

const MIN_AXIS: u32 = 1;
pub(crate) const MAX_STORED_RENDITIONS: usize = 7;
const MAX_FULL_WIDTH_PIXELS: u64 = 32_000_000;

#[cfg(test)]
thread_local! {
    static STORED_DECODE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static RENDITION_ENCODE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_stored_decode_count() {
    STORED_DECODE_COUNT.set(0);
}

#[cfg(test)]
pub(crate) fn stored_decode_count() -> usize {
    STORED_DECODE_COUNT.get()
}

fn record_stored_decode() {
    #[cfg(test)]
    STORED_DECODE_COUNT.set(STORED_DECODE_COUNT.get() + 1);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub url: Url,
    pub alt: Option<String>,
}

/// Status badges remain readable body images, but are not article artwork.
pub fn is_status_badge(source: &str) -> bool {
    fn matches(source: &str, allow_proxy: bool) -> bool {
        let Ok(url) = Url::parse(source) else {
            return false;
        };
        let host = url.host_str().unwrap_or_default();
        let path = url.path();
        match host {
            "github.com" => {
                path.ends_with("/badge.svg")
                    && (path.contains("/actions/") || path.contains("/workflows/"))
            }
            "img.shields.io" | "shields.io" | "badgen.net" | "badge.fury.io" => true,
            "repology.org" => path.starts_with("/badge/"),
            "camo.githubusercontent.com" if allow_proxy => path
                .rsplit('/')
                .next()
                .filter(|value| value.len() <= 8192)
                .and_then(|value| hex::decode(value).ok())
                .and_then(|value| String::from_utf8(value).ok())
                .is_some_and(|value| matches(&value, false)),
            _ => false,
        }
    }
    matches(source, true)
}

#[derive(Debug, Clone)]
pub struct MediaLimits {
    /// Maximum bytes accepted from one HTTP response and one generated rendition.
    pub max_file_bytes: usize,
    /// Maximum cumulative downloaded bytes and cumulative retained bytes for one article.
    pub max_article_bytes: usize,
    pub max_candidates: usize,
    pub max_assets: usize,
    pub max_pixels: u64,
    pub max_axis: u32,
    /// Maximum image responses retained in memory while waiting for a decoder slot.
    pub download_concurrency: usize,
    pub decode_concurrency: usize,
    pub decode_timeout: Duration,
    pub rendition_widths: Vec<u32>,
}

impl Default for MediaLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 32 * 1024 * 1024,
            max_article_bytes: 256 * 1024 * 1024,
            max_candidates: 1024,
            max_assets: 512,
            max_pixels: 200_000_000,
            max_axis: 24_000,
            download_concurrency: 8,
            decode_concurrency: 1,
            decode_timeout: Duration::from_secs(15),
            rendition_widths: vec![320, 640, 960, 1280, 1600],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendition {
    pub bytes: Vec<u8>,
    pub extension: &'static str,
    pub hash: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub source_url: String,
    pub source_hash: String,
    pub alt: Option<String>,
    /// Exact raster response bytes, or a passive PNG rasterized from publisher SVG.
    pub master_bytes: Vec<u8>,
    pub master_extension: &'static str,
    pub master_hash: String,
    /// Intrinsic dimensions after applying the master's orientation metadata.
    pub width: u32,
    pub height: u32,
    /// Deterministic opaque CSS color derived from the oriented image's visible pixels.
    pub dominant_color: String,
    /// Immediate inline preview generated from the oriented master.
    pub placeholder: placeholder::Placeholder,
    /// Ascending by width. Empty for animation, ICC/high-depth input, or small images.
    pub renditions: Vec<Rendition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredKind {
    Master,
    Rendition,
}

#[derive(Debug, Clone, Copy)]
pub struct AssetPart<'a> {
    pub bytes: &'a [u8],
    pub extension: &'static str,
    pub hash: &'a str,
    pub width: u32,
    pub height: u32,
    pub kind: StoredKind,
}

#[derive(Debug)]
pub struct Companion<'a> {
    pub metadata: ImageFile,
    pub bytes: &'a [u8],
    pub kind: StoredKind,
}

impl Asset {
    pub fn metadata(&self, stem: &str) -> ArticleImage {
        let original = AssetPart {
            bytes: &self.master_bytes,
            extension: self.master_extension,
            hash: &self.master_hash,
            width: self.width,
            height: self.height,
            kind: StoredKind::Master,
        };
        ArticleImage {
            source: self.source_url.clone(),
            original: image_file(stem, original),
            variants: self
                .renditions
                .iter()
                .map(|rendition| {
                    image_file(
                        stem,
                        AssetPart {
                            bytes: &rendition.bytes,
                            extension: rendition.extension,
                            hash: &rendition.hash,
                            width: rendition.width,
                            height: rendition.height,
                            kind: StoredKind::Rendition,
                        },
                    )
                })
                .collect(),
            color: Some(self.dominant_color.clone()),
        }
    }

    /// Master first, followed by responsive renditions in ascending width order.
    pub fn renditions_with_master(&self) -> impl Iterator<Item = AssetPart<'_>> {
        std::iter::once(AssetPart {
            bytes: &self.master_bytes,
            extension: self.master_extension,
            hash: &self.master_hash,
            width: self.width,
            height: self.height,
            kind: StoredKind::Master,
        })
        .chain(self.renditions.iter().map(|rendition| AssetPart {
            bytes: &rendition.bytes,
            extension: rendition.extension,
            hash: &rendition.hash,
            width: rendition.width,
            height: rendition.height,
            kind: StoredKind::Rendition,
        }))
    }

    /// Owned filenames plus borrowed bytes, ready for atomic store writes.
    pub fn files(&self, stem: &str) -> Vec<Companion<'_>> {
        self.renditions_with_master()
            .map(|part| Companion {
                metadata: image_file(stem, part),
                bytes: part.bytes,
                kind: part.kind,
            })
            .collect()
    }

    /// Reconstruct an already archived asset without contacting its publisher. The exact master
    /// is required; optional renditions are retained only when they match a fresh resize of it.
    pub fn from_stored(
        metadata: &ArticleImage,
        master: Vec<u8>,
        variants: Vec<Vec<u8>>,
    ) -> Result<Self> {
        ensure!(
            variants.len() == metadata.variants.len(),
            "stored article image rendition count does not match metadata"
        );
        let (stem, _) = metadata
            .original
            .file
            .rsplit_once(".image-")
            .context("stored article image has no owning item")?;
        ensure!(
            metadata.is_valid_for(stem),
            "stored article image metadata is invalid"
        );
        let limits = MediaLimits::default();
        ensure!(
            metadata.variants.len() <= MAX_STORED_RENDITIONS,
            "stored article image has too many renditions"
        );
        let DecodedStored {
            extension: master_extension,
            image: master_image,
            original_color,
            has_icc,
            hash: master_hash,
            animated,
            ..
        } = decode_stored_master(&master, &metadata.original, &limits)?;
        let renditions_allowed = !animated && !has_icc && safe_for_renditions(original_color);

        let mut retained = master.len();
        let mut renditions = Vec::with_capacity(variants.len());
        for (bytes, file) in variants.into_iter().zip(&metadata.variants) {
            let restored = (|| -> Result<(Rendition, usize)> {
                ensure!(
                    renditions_allowed,
                    "stored article image master must not have renditions"
                );
                let DecodedStored {
                    image: decoded_image,
                    hash,
                    ..
                } = decode_stored(&bytes, file, StoredKind::Rendition, &limits)?;
                ensure!(
                    bytes.len() < master.len(),
                    "stored article image rendition is not smaller than its master"
                );
                let next_retained = retained
                    .checked_add(bytes.len())
                    .context("stored article image exceeds article limit")?;
                ensure!(
                    next_retained <= limits.max_article_bytes,
                    "stored article image exceeds article limit"
                );
                let expected = if file.width == master_image.width() {
                    master_image.to_rgba8()
                } else {
                    master_image
                        .resize_exact(
                            file.width,
                            file.height,
                            image::imageops::FilterType::Lanczos3,
                        )
                        .to_rgba8()
                };
                ensure!(
                    expected.dimensions() == (file.width, file.height),
                    "stored article image rendition has an invalid aspect ratio"
                );
                let decoded = decoded_image.to_rgba8();
                ensure!(
                    decoded.as_raw() == expected.as_raw(),
                    "stored article image rendition does not match its master"
                );
                Ok((
                    Rendition {
                        hash,
                        bytes,
                        extension: "webp",
                        width: file.width,
                        height: file.height,
                    },
                    next_retained,
                ))
            })();
            match restored {
                Ok((rendition, next_retained)) => {
                    retained = next_retained;
                    renditions.push(rendition);
                }
                Err(error) => log::debug!(
                    "ignoring invalid stored article image rendition {}: {error:#}",
                    file.file
                ),
            }
        }

        ensure!(
            retained <= limits.max_article_bytes,
            "stored article image exceeds article limit"
        );
        let color = metadata
            .color
            .clone()
            .unwrap_or_else(|| dominant_color(&master_image));
        Ok(Self {
            source_url: metadata.source.clone(),
            source_hash: crate::model::sha1_hex(metadata.source.as_bytes()),
            alt: None,
            master_hash,
            master_bytes: master,
            master_extension,
            width: metadata.original.width,
            height: metadata.original.height,
            dominant_color: color,
            placeholder: placeholder::from_image(&master_image)?,
            renditions,
        })
    }
}

const MAX_ASSET_RECEIPT_BYTES: usize = 8 * 1024;
const ASSET_RECEIPT_SLOTS: usize = 16_384;
const ASSET_RECEIPT_WAYS: usize = 4;

/// Local validation receipts contain no master pixels. Fixed slots bound disk usage without
/// scanning a large cache; collisions only cause a fresh validation, never incorrect reuse.
pub(crate) struct StoredAssetCache {
    root: PathBuf,
    slots: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssetReceipt {
    key: String,
    checksum: String,
    value: ValidatedAsset,
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

impl StoredAssetCache {
    pub(crate) fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().join("validated-images-v2"),
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
            && let Ok(value) = self.read(key)
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

    fn slots(&self, key: &str) -> Result<Vec<PathBuf>> {
        let prefix = key.get(..4).context("invalid image cache key")?;
        let capacity = self.slots.max(1);
        let bucket =
            usize::from(u16::from_str_radix(prefix, 16)?) % capacity.div_ceil(ASSET_RECEIPT_WAYS);
        let first = bucket * ASSET_RECEIPT_WAYS;
        Ok((first..(first + ASSET_RECEIPT_WAYS).min(capacity))
            .map(|slot| self.root.join(format!("{slot:04x}.json")))
            .collect())
    }

    fn read(&self, key: &str) -> Result<ValidatedAsset> {
        self.slots(key)?
            .into_iter()
            .find_map(|path| Self::read_slot(&path, key).ok())
            .context("image validation receipt missing")
    }

    fn read_slot(path: &Path, key: &str) -> Result<ValidatedAsset> {
        let receipt = Self::receipt(path)?;
        ensure!(receipt.key == key, "image cache slot changed");
        Ok(receipt.value)
    }

    fn receipt(path: &Path) -> Result<AssetReceipt> {
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
        let receipt: AssetReceipt = serde_json::from_slice(&bytes)?;
        ensure!(
            receipt.checksum == hex::encode(Sha256::digest(serde_json::to_vec(&receipt.value)?)),
            "image cache receipt checksum changed"
        );
        Ok(receipt)
    }

    fn replacement(&self, key: &str) -> Result<PathBuf> {
        let mut oldest = None;
        for path in self.slots(key)? {
            match Self::receipt(&path) {
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
        let checksum = hex::encode(Sha256::digest(serde_json::to_vec(&value)?));
        let bytes = serde_json::to_vec(&AssetReceipt {
            key: key.into(),
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
        temporary.persist(self.replacement(key)?)?;
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

fn implementation_fingerprint() -> &'static [u8; 32] {
    static IMPLEMENTATION: OnceLock<[u8; 32]> = OnceLock::new();
    IMPLEMENTATION.get_or_init(|| {
        let mut hash = Sha256::new();
        hash.update(include_bytes!("media.rs"));
        hash.update(include_bytes!("media/placeholder.rs"));
        hash.update(include_bytes!("media/srcset.rs"));
        hash.update(include_bytes!("media/vector.rs"));
        hash.update(include_bytes!(
            "media/fonts/AtkinsonHyperlegible-Regular.ttf"
        ));
        hash.update(include_bytes!("../Cargo.lock"));
        hash.finalize().into()
    })
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

fn image_file(stem: &str, part: AssetPart<'_>) -> ImageFile {
    let short_hash = part.hash.get(..12).unwrap_or(part.hash);
    ImageFile {
        file: format!("{stem}.image-{short_hash}.{}", part.extension),
        width: part.width,
        height: part.height,
    }
}

/// A publisher's lead image can sit outside Readability's article subtree. Retain the first
/// explicit/metadata candidate as a full image, using the same limits and URL rules as body media.
pub fn article_candidates(
    html: &str,
    previews: &[crate::preview::Candidate],
    base: &Url,
) -> Vec<Candidate> {
    let lead = previews.iter().find_map(|candidate| {
        safe_image_url(&candidate.url, base).map(|url| Candidate {
            url,
            alt: clean_alt(candidate.alt.as_deref()),
        })
    });
    let mut seen = BTreeSet::new();
    lead.into_iter()
        .chain(body_candidates(html, base))
        .filter(|candidate| seen.insert(candidate.url.clone()))
        .collect()
}

/// Extract safe body-image candidates in document order. The URL fragment is irrelevant to an
/// image request and is removed before deduplication.
pub fn body_candidates(html: &str, base: &Url) -> Vec<Candidate> {
    let normalized = crate::content::normalize_image_sources(html);
    let document = Html::parse_fragment(&normalized);
    let Ok(selector) = Selector::parse("img") else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    document
        .select(&selector)
        .filter_map(|image| {
            let value = ["src", "data-src", "data-lazy-src"]
                .into_iter()
                .filter_map(|attribute| image.value().attr(attribute))
                .find(|value| !value.trim().is_empty() && !is_data_url(value))?;
            let url = safe_image_url(value, base)?;
            let key = url.as_str().to_string();
            seen.insert(key).then(|| Candidate {
                url,
                alt: clean_alt(image.value().attr("alt")),
            })
        })
        .collect()
}

/// Extract only real Markdown images, excluding examples inside fenced or inline code.
pub fn markdown_candidates(markdown: &str, base: &Url) -> Vec<Candidate> {
    if !markdown.contains("![") && !markdown.contains("<img") {
        return Vec::new();
    }
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, markdown, &comrak::Options::default());
    let mut candidates = Vec::new();
    for node in root.descendants() {
        let data = node.data.borrow();
        match &data.value {
            comrak::nodes::NodeValue::Image(link) => {
                if let Some(url) = safe_image_url(&link.url, base) {
                    candidates.push(Candidate { url, alt: None });
                }
            }
            comrak::nodes::NodeValue::HtmlBlock(block) => {
                candidates.extend(body_candidates(&block.literal, base));
            }
            comrak::nodes::NodeValue::HtmlInline(html) => {
                candidates.extend(body_candidates(html, base));
            }
            _ => {}
        }
    }
    let mut seen = BTreeSet::new();
    candidates.retain(|candidate| seen.insert(candidate.url.clone()));
    candidates
}

fn is_data_url(value: &str) -> bool {
    value
        .trim_start_matches(|ch: char| ch.is_whitespace() || ch.is_control())
        .to_ascii_lowercase()
        .starts_with("data:")
}

fn safe_image_url(value: &str, base: &Url) -> Option<Url> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('#') {
        return None;
    }
    let mut url = base.join(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let had_fragment = url.fragment().is_some();
    url.set_fragment(None);
    if had_fragment {
        let mut document = base.clone();
        document.set_fragment(None);
        if url == document {
            return None;
        }
    }
    Some(url)
}

/// Recover the exact comma-split CDN tail emitted by older extraction, retaining the archived
/// URL as the asset identity so Markdown and its local-image substitution remain unchanged.
fn image_request_url(url: &Url) -> Option<Url> {
    safe_image_url(url.as_str(), url)?;
    let Some((_, encoded)) = url.path().split_once("/fl_progressive:steep/") else {
        return Some(url.clone());
    };
    decode_image_origin(encoded)
}

fn decode_image_origin(encoded: &str) -> Option<Url> {
    let mut parts = encoded.split('%');
    let mut decoded = parts.next()?.as_bytes().to_vec();
    for part in parts {
        if part.len() < 2 {
            return None;
        }
        let digits = std::str::from_utf8(&part.as_bytes()[..2]).ok()?;
        let byte = u8::from_str_radix(digits, 16).ok()?;
        decoded.push(byte);
        decoded.extend_from_slice(&part.as_bytes()[2..]);
    }
    let decoded = String::from_utf8(decoded).ok()?;
    let recovered = Url::parse(&decoded).ok()?;
    safe_image_url(recovered.as_str(), &recovered)
}

fn clean_alt(value: Option<&str>) -> Option<String> {
    value
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(300).collect())
}

/// Old comma-split source sets sometimes retained only a CDN transform such as q_auto:good.
/// Recover it only when the saved source set identifies exactly one original image.
fn archived_image_request_url(url: &Url, html: &str, base: &Url) -> Option<Url> {
    let token = url.path().rsplit('/').next()?;
    if url.origin() != base.origin()
        || url.query().is_some()
        || !["w_", "h_", "c_", "f_", "q_", "fl_"]
            .iter()
            .any(|prefix| token.starts_with(prefix))
        || base.join(token).ok().as_ref() != Some(url)
    {
        return None;
    }
    let document = Html::parse_fragment(html);
    let selector = Selector::parse("img, picture source").ok()?;
    let mut originals = BTreeSet::new();
    for element in document.select(&selector) {
        for srcset in ["srcset", "data-srcset", "data-lazy-srcset"]
            .into_iter()
            .filter_map(|name| element.value().attr(name))
        {
            for candidate in srcset::candidates(srcset) {
                let Some(full) = safe_image_url(candidate.url, base) else {
                    continue;
                };
                if full.host_str() != Some("substackcdn.com")
                    || !full.path().starts_with("/image/fetch/")
                    || !candidate
                        .url
                        .split(',')
                        .skip(1)
                        .any(|part| safe_image_url(part, base).as_ref() == Some(url))
                {
                    continue;
                }
                let (_, encoded) = full.path().split_once("fl_progressive:steep/")?;
                originals.insert(decode_image_origin(encoded)?);
            }
        }
    }
    (originals.len() == 1)
        .then(|| originals.pop_first())
        .flatten()
}

pub struct Fetcher {
    client: http::Client,
    download_limit: Arc<Semaphore>,
    decode_limit: Arc<Semaphore>,
    limits: MediaLimits,
    failure_cache: Option<PathBuf>,
}

impl Fetcher {
    pub fn new(fetch: &FetchConfig, limits: MediaLimits) -> Result<Self> {
        ensure!(
            limits.max_file_bytes > 0,
            "media file limit must be positive"
        );
        ensure!(
            limits.max_article_bytes >= limits.max_file_bytes,
            "media article limit must be at least the file limit"
        );
        ensure!(
            limits.max_candidates > 0,
            "media candidate limit must be positive"
        );
        ensure!(limits.max_assets > 0, "media asset limit must be positive");
        ensure!(limits.max_axis >= MIN_AXIS, "media axis limit is too small");
        ensure!(limits.max_pixels > 0, "media pixel limit must be positive");
        ensure!(
            limits.download_concurrency > 0,
            "media download concurrency must be positive"
        );
        ensure!(
            limits.decode_concurrency > 0,
            "media decode concurrency must be positive"
        );
        let client = http::Client::new(&FetchConfig {
            timeout_secs: fetch.timeout_secs,
            max_body_bytes: limits.max_file_bytes,
            retries: fetch.retries,
            ..FetchConfig::default()
        })?;
        Ok(Self {
            client,
            download_limit: Arc::new(Semaphore::new(limits.download_concurrency)),
            decode_limit: Arc::new(Semaphore::new(limits.decode_concurrency)),
            limits,
            failure_cache: None,
        })
    }

    pub fn with_cache(mut self, directory: &Path) -> Self {
        static GENERATION: OnceLock<String> = OnceLock::new();
        let generation = GENERATION.get_or_init(|| hex::encode(implementation_fingerprint()));
        self.failure_cache = Some(directory.join("image-failures-v1").join(generation));
        self
    }

    fn failure_path(&self, url: &Url, source: &Source) -> Option<PathBuf> {
        let key = format!("{}\n{:?}", url, http::source_headers(source, url));
        self.failure_cache
            .as_ref()
            .map(|root| root.join(crate::model::sha1_hex(key.as_bytes())))
    }

    fn recently_failed(&self, request_url: &Url, source: &Source) -> bool {
        self.failure_path(request_url, source)
            .and_then(|path| std::fs::metadata(path).ok())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age < Duration::from_secs(3600))
    }

    pub(crate) fn recently_failed_archived(
        &self,
        candidate: &Candidate,
        source: &Source,
        html: &str,
        base: Option<&Url>,
    ) -> bool {
        base.and_then(|base| archived_image_request_url(&candidate.url, html, base))
            .or_else(|| image_request_url(&candidate.url))
            .is_some_and(|url| self.recently_failed(&url, source))
    }

    fn remember_failure(
        &self,
        candidate: &Candidate,
        request_url: &Url,
        source: &Source,
        reason: &str,
    ) {
        log::warn!(
            "{}: could not archive image {}: {reason}",
            source.slug,
            candidate.url
        );
        if let Some(path) = self.failure_path(request_url, source) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(path, b"retry after one hour\n");
        }
    }

    /// Fetch candidates in source order. An unavailable or malformed image never prevents later
    /// candidates from succeeding; returned assets retain that deterministic source order.
    #[cfg(test)]
    pub async fn fetch(&self, candidates: &[Candidate], source: &Source) -> Vec<Asset> {
        self.fetch_up_to(
            candidates,
            source,
            self.limits.max_assets,
            self.limits.max_article_bytes,
            &BTreeMap::new(),
        )
        .await
    }

    pub async fn fetch_with_budget(
        &self,
        candidates: &[Candidate],
        source: &Source,
        remaining_bytes: usize,
    ) -> Vec<Asset> {
        self.fetch_up_to(
            candidates,
            source,
            self.limits.max_assets,
            remaining_bytes.min(self.limits.max_article_bytes),
            &BTreeMap::new(),
        )
        .await
    }

    pub async fn fetch_archived_with_budget(
        &self,
        candidates: &[Candidate],
        source: &Source,
        remaining_bytes: usize,
        html: &str,
        base: &Url,
    ) -> Vec<Asset> {
        let request_urls = candidates
            .iter()
            .filter_map(|candidate| {
                archived_image_request_url(&candidate.url, html, base)
                    .map(|url| (candidate.url.clone(), url))
            })
            .collect();
        self.fetch_up_to(
            candidates,
            source,
            self.limits.max_assets,
            remaining_bytes.min(self.limits.max_article_bytes),
            &request_urls,
        )
        .await
    }

    /// Alternative URLs for one image stop after the first usable response.
    pub async fn fetch_first(&self, candidates: &[Candidate], source: &Source) -> Option<Asset> {
        self.fetch_up_to(
            candidates,
            source,
            1,
            self.limits.max_article_bytes,
            &BTreeMap::new(),
        )
        .await
        .pop()
    }

    async fn fetch_up_to(
        &self,
        candidates: &[Candidate],
        source: &Source,
        max_assets: usize,
        max_bytes: usize,
        request_urls: &BTreeMap<Url, Url>,
    ) -> Vec<Asset> {
        let mut assets = Vec::<Asset>::new();
        let mut downloaded = 0_usize;
        let mut retained = 0_usize;
        let mut seen = BTreeSet::new();
        let mut resolved = BTreeMap::<Url, usize>::new();
        for candidate in candidates.iter().take(self.limits.max_candidates) {
            if !seen.insert(&candidate.url) {
                continue;
            }
            let Some(request_url) = request_urls
                .get(&candidate.url)
                .cloned()
                .or_else(|| image_request_url(&candidate.url))
            else {
                continue;
            };
            if self.recently_failed(&request_url, source) {
                continue;
            }
            if assets.len() >= max_assets || downloaded >= max_bytes {
                break;
            }
            if let Some(index) = resolved.get(&request_url) {
                let mut asset = assets[*index].clone();
                asset.source_url = candidate.url.to_string();
                asset.source_hash = crate::model::sha1_hex(candidate.url.as_str().as_bytes());
                if let Some(used) =
                    fit_asset_to_budget(&mut asset, max_bytes.saturating_sub(retained))
                {
                    retained += used;
                    assets.push(asset);
                }
                continue;
            }
            let Ok(download_permit) = self.download_limit.clone().acquire_owned().await else {
                break;
            };
            let response = self
                .client
                .get(http::Request {
                    url: &request_url,
                    headers: http::source_headers(source, &request_url),
                    etag: None,
                    last_modified: None,
                })
                .await;
            let body = match response {
                Ok(http::Response::Ok(body)) => body,
                Ok(_) => {
                    self.remember_failure(
                        candidate,
                        &request_url,
                        source,
                        "unexpected empty response",
                    );
                    continue;
                }
                Err(error) => {
                    self.remember_failure(candidate, &request_url, source, &format!("{error:#}"));
                    continue;
                }
            };
            let Some(next_downloaded) = downloaded.checked_add(body.bytes.len()) else {
                break;
            };
            if next_downloaded > max_bytes {
                break;
            }
            downloaded = next_downloaded;

            let Ok(permit) = self.decode_limit.clone().acquire_owned().await else {
                break;
            };
            drop(download_permit);
            let decoding_candidate = candidate.clone();
            let limits = self.limits.clone();
            let task = tokio::task::spawn_blocking(move || {
                // A timed-out blocking task cannot be cancelled. Keep its shared slot until its
                // decoder really exits so subsequent articles remain bounded.
                let _permit = permit;
                prepare_asset(&decoding_candidate, body.bytes, &limits)
            });
            let result = tokio::time::timeout(self.limits.decode_timeout, task).await;
            let mut asset = match result {
                Ok(Ok(Ok(asset))) => asset,
                Ok(Ok(Err(error))) => {
                    self.remember_failure(candidate, &request_url, source, &format!("{error:#}"));
                    continue;
                }
                Ok(Err(error)) => {
                    self.remember_failure(
                        candidate,
                        &request_url,
                        source,
                        &format!("decoder task failed: {error}"),
                    );
                    continue;
                }
                Err(_) => {
                    self.remember_failure(candidate, &request_url, source, "decoder timed out");
                    continue;
                }
            };

            let remaining = max_bytes.saturating_sub(retained);
            let Some(used) = fit_asset_to_budget(&mut asset, remaining) else {
                continue;
            };
            retained += used;
            if let Some(path) = self.failure_path(&request_url, source) {
                let _ = std::fs::remove_file(path);
            }
            resolved.insert(request_url, assets.len());
            assets.push(asset);
        }
        assets
    }
}

/// Fit optional renditions around the exact master. Large originals remain the full-width
/// source without an additional full-resolution encode; ThumbHash is independent of these files.
fn fit_asset_to_budget(asset: &mut Asset, remaining: usize) -> Option<usize> {
    if asset.master_bytes.len() > remaining {
        return None;
    }
    let mut used = asset.master_bytes.len();
    asset.renditions.retain(|rendition| {
        if rendition.bytes.len() <= remaining.saturating_sub(used) {
            used += rendition.bytes.len();
            true
        } else {
            false
        }
    });
    let has_full_width = asset.renditions.last().is_some_and(|rendition| {
        rendition.width == asset.width && rendition.height == asset.height
    });
    let large = u64::from(asset.width) * u64::from(asset.height) > MAX_FULL_WIDTH_PIXELS;
    if !large && asset.master_extension != "webp" && !has_full_width && !asset.renditions.is_empty()
    {
        asset.renditions.clear();
        used = asset.master_bytes.len();
    }
    Some(used)
}

/// Validate and decode one response, keeping raster bytes (or a passive SVG raster) as the master and deriving only
/// pixel-lossless WebP renditions that fit the configured limits.
pub fn prepare_asset(candidate: &Candidate, bytes: Vec<u8>, limits: &MediaLimits) -> Result<Asset> {
    if bytes.len() > limits.max_file_bytes {
        bail!("image exceeds file limit");
    }
    // SVG markup is never archived as an active document: its passive raster is the master.
    let bytes = if image::guess_format(&bytes).is_err() {
        vector::rasterize(&bytes, limits).context("rasterizing article SVG")?
    } else {
        bytes
    };
    let format = image::guess_format(&bytes).context("recognizing article image")?;
    let extension = match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Png => "png",
        ImageFormat::Gif => "gif",
        ImageFormat::WebP => "webp",
        _ => bail!("unsupported article image format"),
    };
    let animated = is_animated(&bytes, format, limits)?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes.as_slice()), format);
    reader.limits(decoder_limits(limits));
    let mut decoder = reader
        .into_decoder()
        .context("reading article image metadata")?;
    let (encoded_width, encoded_height) = decoder.dimensions();
    validate_dimensions(encoded_width, encoded_height, limits)?;
    let original_color = decoder.original_color_type();
    let has_icc = decoder
        .icc_profile()
        .map_or(true, |profile| profile.is_some());
    let orientation = decoder
        .orientation()
        .context("reading article image orientation")?;
    let mut image = DynamicImage::from_decoder(decoder).context("decoding article image")?;
    image.apply_orientation(orientation);
    let (width, height) = image.dimensions();
    validate_dimensions(width, height, limits)?;

    let rendition_widths = limits
        .rendition_widths
        .iter()
        .copied()
        .filter(|width| *width > 0 && *width < image.width())
        .collect::<BTreeSet<_>>();
    let mut renditions = Vec::with_capacity(rendition_widths.len() + 1);
    if !animated && !has_icc && safe_for_renditions(original_color) {
        // Avoid full-resolution RGBA copies and encoding for very large originals. Their exact
        // master is the widest source alongside bounded lossless renditions.
        let large = u64::from(width) * u64::from(height) > MAX_FULL_WIDTH_PIXELS;
        let full_width = (format != ImageFormat::WebP && !large)
            .then(|| prepare_rendition(&image, image.width(), limits.max_file_bytes, bytes.len()))
            .flatten();
        let responsive = large || format == ImageFormat::WebP || full_width.is_some();
        for rendition_width in rendition_widths.into_iter().filter(|_| responsive) {
            if let Some(rendition) =
                prepare_rendition(&image, rendition_width, limits.max_file_bytes, bytes.len())
            {
                renditions.push(rendition);
            }
        }
        renditions.extend(full_width);
    }
    Ok(Asset {
        source_url: candidate.url.to_string(),
        source_hash: crate::model::sha1_hex(candidate.url.as_str().as_bytes()),
        alt: candidate.alt.clone(),
        master_hash: crate::model::sha1_hex(&bytes),
        master_bytes: bytes,
        master_extension: extension,
        width,
        height,
        dominant_color: dominant_color(&image),
        placeholder: placeholder::from_image(&image)?,
        renditions,
    })
}

fn prepare_rendition(
    image: &DynamicImage,
    width: u32,
    max_file_bytes: usize,
    master_bytes: usize,
) -> Option<Rendition> {
    let resized = if width == image.width() {
        image.to_rgba8()
    } else {
        image
            .resize_exact(
                width,
                scaled_height(image.width(), image.height(), width),
                image::imageops::FilterType::Lanczos3,
            )
            .to_rgba8()
    };
    lossless_webp(&resized, max_file_bytes)
        .ok()
        .filter(|rendition| rendition.bytes.len() < master_bytes)
}

/// Validate an untrusted stored companion using the same conservative bounds as acquisition.
/// The filename is checked against the actual bytes rather than trusted as a MIME declaration.
pub fn validate_stored(bytes: &[u8], file: &ImageFile, kind: StoredKind) -> Result<&'static str> {
    validate_stored_with_limits(bytes, file, kind, &MediaLimits::default())
}

pub fn validate_stored_with_limits(
    bytes: &[u8],
    file: &ImageFile,
    kind: StoredKind,
    limits: &MediaLimits,
) -> Result<&'static str> {
    Ok(decode_stored(bytes, file, kind, limits)?.extension)
}

struct DecodedStored {
    extension: &'static str,
    image: DynamicImage,
    original_color: ExtendedColorType,
    has_icc: bool,
    hash: String,
    animated: bool,
}

fn stored_identity(
    bytes: &[u8],
    file: &ImageFile,
    kind: StoredKind,
    limits: &MediaLimits,
) -> Result<(ImageFormat, &'static str, String)> {
    ensure!(
        bytes.len() <= limits.max_file_bytes,
        "stored article image exceeds file limit"
    );
    stored_identity_with_hash(bytes, file, kind, limits, crate::model::sha1_hex(bytes))
}

// A matching receipt binds these validated identities to every current byte through its
// SHA-256 key; the checksum detects receipt corruption. Rechecking SHA-1 repeats that byte pass.
fn stored_identity_with_hash(
    bytes: &[u8],
    file: &ImageFile,
    kind: StoredKind,
    limits: &MediaLimits,
    hash: String,
) -> Result<(ImageFormat, &'static str, String)> {
    ensure!(
        hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid stored image digest"
    );
    if bytes.len() > limits.max_file_bytes {
        bail!("stored article image exceeds file limit");
    }
    let format = image::guess_format(bytes).context("recognizing stored article image")?;
    let extension = match format {
        ImageFormat::Jpeg if kind == StoredKind::Master => "jpg",
        ImageFormat::Png if kind == StoredKind::Master => "png",
        ImageFormat::Gif if kind == StoredKind::Master => "gif",
        ImageFormat::WebP => "webp",
        _ => bail!("stored article image has an invalid format for its role"),
    };
    ensure!(
        !file.file.contains(['/', '\\', ':']) && !file.file.starts_with('.'),
        "stored article image companion name is unsafe"
    );
    let (owner, suffix) = file
        .file
        .rsplit_once(".image-")
        .context("stored article image has an invalid companion name")?;
    ensure!(!owner.is_empty(), "stored article image has no owner");
    let (short_hash, declared_extension) = suffix
        .rsplit_once('.')
        .context("stored article image has no extension")?;
    ensure!(
        short_hash == &hash[..12] && declared_extension == extension,
        "stored article image filename does not match its bytes"
    );
    if kind == StoredKind::Rendition {
        ensure!(
            webp_is_lossless(bytes),
            "stored article image rendition is not lossless WebP"
        );
    }

    Ok((format, extension, hash))
}

fn decode_stored(
    bytes: &[u8],
    file: &ImageFile,
    kind: StoredKind,
    limits: &MediaLimits,
) -> Result<DecodedStored> {
    let (format, extension, hash) = stored_identity(bytes, file, kind, limits)?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(decoder_limits(limits));
    let decoder = reader
        .into_decoder()
        .context("reading stored article image metadata")?;
    decode_stored_with_decoder(decoder, file, extension, hash, false, limits)
}

fn decode_stored_master(
    bytes: &[u8],
    file: &ImageFile,
    limits: &MediaLimits,
) -> Result<DecodedStored> {
    let (format, extension, hash) = stored_identity(bytes, file, StoredKind::Master, limits)?;
    match format {
        ImageFormat::Gif => decode_stored_gif(bytes, file, extension, hash, limits),
        ImageFormat::Png => {
            let decoder = image::codecs::png::PngDecoder::with_limits(
                Cursor::new(bytes),
                decoder_limits(limits),
            )?;
            let animated = decoder.is_apng()?;
            decode_stored_with_decoder(decoder, file, extension, hash, animated, limits)
        }
        ImageFormat::WebP => {
            let mut decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))?;
            decoder.set_limits(decoder_limits(limits))?;
            let animated = decoder.has_animation();
            decode_stored_with_decoder(decoder, file, extension, hash, animated, limits)
        }
        ImageFormat::Jpeg => {
            let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
            reader.limits(decoder_limits(limits));
            let decoder = reader
                .into_decoder()
                .context("reading stored article image metadata")?;
            decode_stored_with_decoder(decoder, file, extension, hash, false, limits)
        }
        _ => bail!("stored article image has an invalid master format"),
    }
}

fn decode_stored_with_decoder(
    mut decoder: impl image::ImageDecoder,
    file: &ImageFile,
    extension: &'static str,
    hash: String,
    animated: bool,
    limits: &MediaLimits,
) -> Result<DecodedStored> {
    decoder.set_limits(decoder_limits(limits))?;
    let (encoded_width, encoded_height) = decoder.dimensions();
    validate_stored_dimensions(encoded_width, encoded_height, limits)?;
    let original_color = decoder.original_color_type();
    let has_icc = decoder
        .icc_profile()
        .map_or(true, |profile| profile.is_some());
    let orientation = decoder
        .orientation()
        .context("reading stored article image orientation")?;
    record_stored_decode();
    let mut image = DynamicImage::from_decoder(decoder).context("decoding stored article image")?;
    image.apply_orientation(orientation);
    validate_stored_dimensions(image.width(), image.height(), limits)?;
    ensure!(
        (image.width(), image.height()) == (file.width, file.height),
        "stored article image dimensions do not match metadata"
    );
    Ok(DecodedStored {
        extension,
        image,
        original_color,
        has_icc,
        hash,
        animated,
    })
}

fn decode_stored_gif(
    bytes: &[u8],
    file: &ImageFile,
    extension: &'static str,
    hash: String,
    limits: &MediaLimits,
) -> Result<DecodedStored> {
    let mut decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))?;
    decoder.set_limits(decoder_limits(limits))?;
    let (encoded_width, encoded_height) = decoder.dimensions();
    validate_stored_dimensions(encoded_width, encoded_height, limits)?;
    let original_color = decoder.original_color_type();
    let has_icc = decoder
        .icc_profile()
        .map_or(true, |profile| profile.is_some());
    let orientation = decoder
        .orientation()
        .context("reading stored article image orientation")?;
    record_stored_decode();
    let mut frames = decoder.into_frames();
    let first = frames
        .next()
        .transpose()?
        .context("stored GIF has no image frame")?;
    let animated = frames.next().transpose()?.is_some();
    let mut image = DynamicImage::ImageRgba8(first.into_buffer());
    image.apply_orientation(orientation);
    validate_stored_dimensions(image.width(), image.height(), limits)?;
    ensure!(
        image.dimensions() == (file.width, file.height),
        "stored article image dimensions do not match metadata"
    );
    Ok(DecodedStored {
        extension,
        image,
        original_color,
        has_icc,
        hash,
        animated,
    })
}

fn decoder_limits(limits: &MediaLimits) -> image::Limits {
    let mut decoder = image::Limits::default();
    decoder.max_image_width = Some(limits.max_axis);
    decoder.max_image_height = Some(limits.max_axis);
    decoder.max_alloc = Some(limits.max_pixels.saturating_mul(8).min(768 * 1024 * 1024));
    decoder
}

fn validate_dimensions(width: u32, height: u32, limits: &MediaLimits) -> Result<()> {
    if width < MIN_AXIS
        || height < MIN_AXIS
        || width > limits.max_axis
        || height > limits.max_axis
        || u64::from(width) * u64::from(height) > limits.max_pixels
    {
        bail!(
            "article image dimensions {width}x{height} exceed limits ({} pixels, {} per axis)",
            limits.max_pixels,
            limits.max_axis
        );
    }
    Ok(())
}

fn validate_stored_dimensions(width: u32, height: u32, limits: &MediaLimits) -> Result<()> {
    if width == 0
        || height == 0
        || width > limits.max_axis
        || height > limits.max_axis
        || u64::from(width) * u64::from(height) > limits.max_pixels
    {
        bail!(
            "stored article image dimensions {width}x{height} exceed limits ({} pixels, {} per axis)",
            limits.max_pixels,
            limits.max_axis
        );
    }
    Ok(())
}

fn is_animated(bytes: &[u8], format: ImageFormat, limits: &MediaLimits) -> Result<bool> {
    match format {
        ImageFormat::Gif => {
            let mut decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))?;
            decoder.set_limits(decoder_limits(limits))?;
            let mut frames = decoder.into_frames();
            let first = frames.next().transpose()?;
            let second = frames.next().transpose()?;
            Ok(first.is_some() && second.is_some())
        }
        ImageFormat::WebP => {
            let mut decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))?;
            decoder.set_limits(decoder_limits(limits))?;
            Ok(decoder.has_animation())
        }
        ImageFormat::Png => {
            let mut decoder = image::codecs::png::PngDecoder::new(Cursor::new(bytes))?;
            decoder.set_limits(decoder_limits(limits))?;
            Ok(decoder.is_apng()?)
        }
        _ => Ok(false),
    }
}

fn safe_for_renditions(color: ExtendedColorType) -> bool {
    matches!(
        color,
        ExtendedColorType::A8
            | ExtendedColorType::L1
            | ExtendedColorType::La1
            | ExtendedColorType::Rgb1
            | ExtendedColorType::Rgba1
            | ExtendedColorType::L2
            | ExtendedColorType::La2
            | ExtendedColorType::Rgb2
            | ExtendedColorType::Rgba2
            | ExtendedColorType::L4
            | ExtendedColorType::La4
            | ExtendedColorType::Rgb4
            | ExtendedColorType::Rgba4
            | ExtendedColorType::Rgb5x1
            | ExtendedColorType::L8
            | ExtendedColorType::La8
            | ExtendedColorType::Rgb8
            | ExtendedColorType::Rgba8
            | ExtendedColorType::Bgr8
            | ExtendedColorType::Bgra8
    )
}

fn scaled_height(width: u32, height: u32, target_width: u32) -> u32 {
    ((u64::from(height) * u64::from(target_width) + u64::from(width) / 2) / u64::from(width)).max(1)
        as u32
}

fn lossless_webp(image: &image::RgbaImage, max_bytes: usize) -> Result<Rendition> {
    #[cfg(test)]
    RENDITION_ENCODE_COUNT.set(RENDITION_ENCODE_COUNT.get() + 1);
    let mut bytes = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut bytes)
        .encode(
            image,
            image.width(),
            image.height(),
            ExtendedColorType::Rgba8,
        )
        .context("encoding lossless article image rendition")?;
    if bytes.len() > max_bytes {
        bail!("article image rendition exceeds file limit");
    }
    let decoded = image::load_from_memory_with_format(&bytes, ImageFormat::WebP)
        .context("verifying article image rendition")?
        .to_rgba8();
    ensure!(
        decoded.dimensions() == image.dimensions() && decoded.as_raw() == image.as_raw(),
        "lossless WebP rendition failed pixel verification"
    );
    Ok(Rendition {
        hash: crate::model::sha1_hex(&bytes),
        bytes,
        extension: "webp",
        width: image.width(),
        height: image.height(),
    })
}

fn webp_is_lossless(bytes: &[u8]) -> bool {
    if bytes.len() < 20 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return false;
    }
    let mut position = 12_usize;
    while position
        .checked_add(8)
        .is_some_and(|end| end <= bytes.len())
    {
        let tag = &bytes[position..position + 4];
        let size = u32::from_le_bytes([
            bytes[position + 4],
            bytes[position + 5],
            bytes[position + 6],
            bytes[position + 7],
        ]) as usize;
        let Some(end) = position
            .checked_add(8)
            .and_then(|start| start.checked_add(size))
        else {
            return false;
        };
        if end > bytes.len() {
            return false;
        }
        if tag == b"VP8L" {
            return true;
        }
        if tag == b"VP8 " || tag == b"ANIM" {
            return false;
        }
        let Some(next) = end.checked_add(size & 1) else {
            return false;
        };
        position = next;
    }
    false
}

fn dominant_color(image: &DynamicImage) -> String {
    let sample = if image.width() > 64 || image.height() > 64 {
        image.resize(64, 64, image::imageops::FilterType::Triangle)
    } else {
        image.clone()
    }
    .to_rgba8();
    let mut buckets = BTreeMap::<u16, (u64, u64, u64, u64)>::new();
    for pixel in sample.pixels() {
        let [red, green, blue, alpha] = pixel.0;
        if alpha == 0 {
            continue;
        }
        let key = (u16::from(red >> 4) << 8) | (u16::from(green >> 4) << 4) | u16::from(blue >> 4);
        let weight = u64::from(alpha);
        let bucket = buckets.entry(key).or_default();
        bucket.0 += weight;
        bucket.1 += u64::from(red) * weight;
        bucket.2 += u64::from(green) * weight;
        bucket.3 += u64::from(blue) * weight;
    }
    let (_, (weight, red, green, blue)) = buckets
        .into_iter()
        .max_by_key(|(key, (weight, _, _, _))| (*weight, std::cmp::Reverse(*key)))
        .unwrap_or((0, (1, 0, 0, 0)));
    format!(
        "#{:02x}{:02x}{:02x}",
        red / weight,
        green / weight,
        blue / weight
    )
}

#[cfg(test)]
mod tests {

    use super::*;
    use image::{ImageBuffer, ImageEncoder as _, Rgba};

    fn png(image: &DynamicImage) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    fn jpeg(image: &DynamicImage, quality: u8) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode_image(image)
            .unwrap();
        bytes
    }

    fn png_with_icc(image: &image::RgbaImage) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
        encoder.set_icc_profile(vec![0_u8; 128]).unwrap();
        encoder
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                ExtendedColorType::Rgba8,
            )
            .unwrap();
        bytes
    }

    fn candidate(url: &str) -> Candidate {
        Candidate {
            url: Url::parse(url).unwrap(),
            alt: Some("An image".into()),
        }
    }

    #[test]
    fn image_candidates_exclude_empty_sources_and_article_footnotes() {
        let base = Url::parse("https://mitchellh.com/writing/libghostty-is-coming").unwrap();
        let markdown = "emulation![1](https://mitchellh.com/writing/libghostty-is-coming#user-content-fn-1)\n![2](#fn-2)\n![Empty]()\n![Chart](/chart.svg#view)";
        let html = r##"<img src=""><img src="#fn-1"><img src="/writing/libghostty-is-coming#fn-2"><img src="/chart.svg#view">"##;
        for candidates in [
            markdown_candidates(markdown, &base),
            body_candidates(html, &base),
        ] {
            assert_eq!(
                candidates
                    .iter()
                    .map(|candidate| candidate.url.as_str())
                    .collect::<Vec<_>>(),
                ["https://mitchellh.com/chart.svg"]
            );
        }
    }

    #[test]
    fn archive_candidates_keep_small_images_and_exclude_code_examples() {
        let base = Url::parse("https://example.com/article/").unwrap();
        let markdown = "![Figure](../one.png)\n![Again](../one.png#x)\n\n```md\n![Example](fake.png)\n```\n\n<img src=\"tiny.png\" width=\"1\" height=\"1\">";
        let urls: Vec<_> = markdown_candidates(markdown, &base)
            .into_iter()
            .map(|candidate| candidate.url.to_string())
            .collect();
        assert_eq!(
            urls,
            [
                "https://example.com/one.png",
                "https://example.com/article/tiny.png"
            ]
        );
    }

    #[test]
    fn article_candidates_keep_a_metadata_lead_without_duplicating_body_images() {
        let base = Url::parse("https://example.com/article").unwrap();
        let previews = vec![
            crate::preview::Candidate {
                url: "javascript:bad".into(),
                alt: None,
            },
            crate::preview::Candidate {
                url: "/lead.jpg#fragment".into(),
                alt: Some("  Remote   control ".into()),
            },
            crate::preview::Candidate {
                url: "/unrelated.jpg".into(),
                alt: None,
            },
        ];
        let images = article_candidates("<p>Story</p><img src='/body.jpg'>", &previews, &base);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].url.as_str(), "https://example.com/lead.jpg");
        assert_eq!(images[0].alt.as_deref(), Some("Remote control"));
        assert_eq!(images[1].url.as_str(), "https://example.com/body.jpg");
        let duplicate = article_candidates(
            "<img src='/lead.jpg'><img src='/body.jpg'>",
            &previews,
            &base,
        );
        assert_eq!(duplicate.len(), 2);
    }

    #[test]
    fn recognizes_status_badges_without_excluding_regular_artwork() {
        let badge =
            "https://github.com/iczelia/bzip3/actions/workflows/build.yml/badge.svg?branch=main";
        assert!(is_status_badge(badge));
        assert!(is_status_badge(
            "https://img.shields.io/badge/build-passing-green"
        ));
        assert!(is_status_badge(&format!(
            "https://camo.githubusercontent.com/hash/{}",
            hex::encode("https://repology.org/badge/vertical-allrepos/bzip3.svg")
        )));
        for source in [
            "https://example.com/badge.svg",
            "https://github.com/user/project/raw/main/diagram.svg",
            "https://camo.githubusercontent.com/hash/invalid",
        ] {
            assert!(!is_status_badge(source));
        }
    }

    #[test]
    fn extracts_ordered_deduplicated_safe_body_images() {
        let base = Url::parse("https://example.com/articles/post").unwrap();
        let html = r#"
          <img src="/first.png" alt="  First   image ">
          <img src="/first.png#duplicate">
          <img src="data:image/png;base64,AAAA">
          <img src="javascript:alert(1)">
          <img src="https://user:secret@example.com/private.png">
          <img src="/vector.SVG?download=1">
          <img src="/pixel.png" width="1" height="1">
          <img src="data:image/gif;base64,x" data-src="second.webp">
          <img src="tiny.jpg" srcset="responsive-small.jpg 320w, responsive-large.jpg 1280w">
          <picture><source type="image/webp" srcset="picture-small.webp 1x, picture-large.webp 2x"><img src="data:image/gif;base64,x"></picture>
        "#;
        assert_eq!(
            body_candidates(html, &base),
            vec![
                Candidate {
                    url: Url::parse("https://example.com/first.png").unwrap(),
                    alt: Some("First image".into()),
                },
                Candidate {
                    url: Url::parse("https://example.com/vector.SVG?download=1").unwrap(),
                    alt: None,
                },
                Candidate {
                    url: Url::parse("https://example.com/pixel.png").unwrap(),
                    alt: None,
                },
                Candidate {
                    url: Url::parse("https://example.com/articles/second.webp").unwrap(),
                    alt: None,
                },
                Candidate {
                    url: Url::parse("https://example.com/articles/responsive-large.jpg").unwrap(),
                    alt: None,
                },
                Candidate {
                    url: Url::parse("https://example.com/articles/picture-large.webp").unwrap(),
                    alt: None,
                },
            ]
        );
    }

    #[test]
    fn preserves_exact_master_and_emits_ordered_non_upscaled_renditions() {
        let source = DynamicImage::ImageRgba8(ImageBuffer::from_fn(800, 480, |x, y| {
            Rgba([(x % 251) as u8, (y % 239) as u8, ((x + y) % 241) as u8, 255])
        }));
        let bytes = png(&source);
        let asset = prepare_asset(
            &candidate("https://example.com/photo.png"),
            bytes.clone(),
            &MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(asset.master_bytes, bytes);
        assert_eq!(asset.master_extension, "png");
        assert_eq!((asset.width, asset.height), (800, 480));
        assert_eq!(
            asset
                .renditions
                .iter()
                .map(|rendition| rendition.width)
                .collect::<Vec<_>>(),
            [320, 640, 800]
        );
        assert!(asset.renditions.iter().all(|rendition| {
            rendition.extension == "webp"
                && rendition.width <= asset.width
                && rendition.hash == crate::model::sha1_hex(&rendition.bytes)
                && rendition.bytes.len() < asset.master_bytes.len()
        }));
        assert_eq!(
            asset.master_hash,
            crate::model::sha1_hex(&asset.master_bytes)
        );

        let metadata = asset.metadata("article");
        assert!(metadata.is_valid_for("article"));
        let files = asset.files("article");
        assert_eq!(files.len(), 4);
        assert_eq!(files[0].metadata, metadata.original);
        assert_eq!(
            files[1..]
                .iter()
                .map(|file| file.metadata.clone())
                .collect::<Vec<_>>(),
            metadata.variants
        );
        for file in files {
            assert_eq!(
                validate_stored(file.bytes, &file.metadata, file.kind).unwrap(),
                file.metadata.file.rsplit_once('.').unwrap().1
            );
        }
        let mirrored = Asset::from_stored(
            &metadata,
            asset.master_bytes.clone(),
            asset
                .renditions
                .iter()
                .map(|rendition| rendition.bytes.clone())
                .collect(),
        )
        .unwrap();
        assert_eq!(mirrored.master_bytes, asset.master_bytes);
        assert_eq!(mirrored.renditions, asset.renditions);
        assert_eq!(mirrored.dominant_color, asset.dominant_color);
    }

    #[test]
    fn alpha_rendition_decodes_to_the_exact_resized_pixels() {
        let source = DynamicImage::ImageRgba8(ImageBuffer::from_fn(400, 240, |x, y| {
            Rgba([
                (x % 255) as u8,
                (y % 255) as u8,
                ((x * 3 + y) % 255) as u8,
                ((x + y) % 255) as u8,
            ])
        }));
        let asset = prepare_asset(
            &candidate("https://example.com/alpha.png"),
            png(&source),
            &MediaLimits::default(),
        )
        .unwrap();
        let rendition = &asset.renditions[0];
        let expected = source
            .resize_exact(
                rendition.width,
                rendition.height,
                image::imageops::FilterType::Lanczos3,
            )
            .to_rgba8();
        let decoded = image::load_from_memory(&rendition.bytes)
            .unwrap()
            .to_rgba8();
        assert_eq!(decoded, expected);
    }

    #[test]
    fn small_images_are_never_upscaled() {
        let image = DynamicImage::new_rgba8(200, 120);
        let asset = prepare_asset(
            &candidate("https://example.com/small.png"),
            png(&image),
            &MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(asset.renditions.len(), 1);
        assert_eq!(
            (asset.renditions[0].width, asset.renditions[0].height),
            (200, 120)
        );

        let below_placeholder = DynamicImage::new_rgba8(40, 36);
        let asset = prepare_asset(
            &candidate("https://example.com/below-placeholder.png"),
            png(&below_placeholder),
            &MediaLimits::default(),
        )
        .unwrap();
        assert!(
            asset
                .renditions
                .iter()
                .all(|rendition| rendition.width <= below_placeholder.width())
        );
    }

    #[test]
    #[ignore = "manual local publisher-image timing; set AGGR_TEST_IMAGE"]
    fn prepare_local_publisher_image() {
        let bytes = std::fs::read(std::env::var("AGGR_TEST_IMAGE").unwrap()).unwrap();
        let started = std::time::Instant::now();
        let asset = prepare_asset(
            &candidate("https://example.com/local-image"),
            bytes.clone(),
            &MediaLimits::default(),
        )
        .unwrap();
        eprintln!(
            "publisher image: {} bytes, {}x{}, renditions {:?}, placeholder {} bytes, elapsed {:?}",
            bytes.len(),
            asset.width,
            asset.height,
            asset.renditions.iter().map(|r| r.width).collect::<Vec<_>>(),
            asset.placeholder.data_url.len(),
            started.elapsed()
        );
        assert_eq!(asset.master_bytes, bytes);
        assert!(!asset.placeholder.hash.is_empty());
    }

    #[test]
    fn svg_source_is_archived_as_a_passive_raster_with_inline_preview() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="80"><rect width="100" height="80" fill="red"/></svg>"#;
        let asset = prepare_asset(
            &candidate("https://example.com/diagram.svg"),
            svg.to_vec(),
            &MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(asset.source_url, "https://example.com/diagram.svg");
        assert_eq!(asset.master_extension, "png");
        assert_eq!(
            image::guess_format(&asset.master_bytes).unwrap(),
            ImageFormat::Png
        );
        assert!(!asset.placeholder.hash.is_empty());
        assert_eq!(
            Asset::from_stored(
                &asset.metadata("diagram"),
                asset.master_bytes.clone(),
                asset.renditions.iter().map(|r| r.bytes.clone()).collect()
            )
            .unwrap()
            .placeholder,
            asset.placeholder
        );
    }

    #[test]
    fn large_chart_dimensions_fit_the_bounded_rgba_decode_budget() {
        let limits = MediaLimits::default();
        for (width, height) in [
            (17_277, 11_171),
            (17_457, 10_428),
            (17_277, 10_523),
            (17_277, 9_669),
            (17_277, 10_123),
        ] {
            validate_dimensions(width, height, &limits).unwrap();
            validate_stored_dimensions(width, height, &limits).unwrap();
            decoder_limits(&limits)
                .reserve(u64::from(width) * u64::from(height) * 4)
                .unwrap();
        }
        for (width, height) in [(20_000, 10_001), (24_001, 1), (0, 1)] {
            assert!(validate_dimensions(width, height, &limits).is_err());
            assert!(validate_stored_dimensions(width, height, &limits).is_err());
        }
        assert!(decoder_limits(&limits).reserve(1024 * 1024 * 1024).is_err());
    }

    #[test]
    fn large_originals_keep_bounded_renditions_without_full_resolution_encode() {
        RENDITION_ENCODE_COUNT.set(0);
        let bytes = jpeg(&DynamicImage::new_rgb8(8001, 4000), 60);
        let limits = MediaLimits {
            rendition_widths: vec![320],
            ..MediaLimits::default()
        };
        let mut asset = prepare_asset(
            &candidate("https://example.com/large.jpg"),
            bytes.clone(),
            &limits,
        )
        .unwrap();
        assert_eq!(asset.master_bytes, bytes);
        assert_eq!(
            asset.renditions.iter().map(|r| r.width).collect::<Vec<_>>(),
            [320]
        );
        assert_eq!(
            RENDITION_ENCODE_COUNT.get(),
            1,
            "never encode a full-resolution copy of a large master"
        );
        assert_eq!(
            fit_asset_to_budget(&mut asset, limits.max_article_bytes),
            Some(bytes.len() + asset.renditions[0].bytes.len())
        );
        assert_eq!(
            placeholder::from_hash(&asset.placeholder.hash).unwrap(),
            asset.placeholder
        );
    }

    #[test]
    fn compact_masters_use_inline_placeholders_without_encoding_incomplete_sources() {
        RENDITION_ENCODE_COUNT.set(0);
        let source = DynamicImage::ImageRgb8(ImageBuffer::from_fn(800, 480, |x, y| {
            image::Rgb([
                ((x * 37 + y * 11) % 256) as u8,
                ((x * 13 + y * 41) % 256) as u8,
                ((x * 29 + y * 23) % 256) as u8,
            ])
        }));
        let bytes = jpeg(&source, 55);
        let asset = prepare_asset(
            &candidate("https://example.com/already-compact.jpg"),
            bytes.clone(),
            &MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(asset.master_bytes, bytes);
        assert!(asset.renditions.is_empty());
        assert_eq!(
            placeholder::from_hash(&asset.placeholder.hash).unwrap(),
            asset.placeholder
        );
        assert_eq!(
            RENDITION_ENCODE_COUNT.get(),
            1,
            "only test the full-width candidate"
        );
        let metadata = asset.metadata("compact");
        Asset::from_stored(
            &metadata,
            asset.master_bytes.clone(),
            asset
                .renditions
                .iter()
                .map(|rendition| rendition.bytes.clone())
                .collect(),
        )
        .unwrap();
    }

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
            .slots(&key)
            .unwrap()
            .into_iter()
            .find(|path| path.is_file())
            .unwrap();
        let mut receipt = StoredAssetCache::receipt(&path).unwrap();
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
        let mut invalid: AssetReceipt =
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
            let mut invalid = StoredAssetCache::receipt(&receipt).unwrap();
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
    fn stored_master_derives_an_inline_placeholder_without_rewriting_it() {
        let source = DynamicImage::ImageRgb8(ImageBuffer::from_fn(640, 320, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 239) as u8, ((x + y) % 241) as u8])
        }));
        let bytes = png(&source);
        let prepared = prepare_asset(
            &candidate("https://example.com/legacy.png"),
            bytes.clone(),
            &MediaLimits::default(),
        )
        .unwrap();
        let mut metadata = prepared.metadata("legacy");
        metadata.variants.clear();

        let restored = Asset::from_stored(&metadata, bytes.clone(), vec![]).unwrap();
        assert_eq!(restored.master_bytes, bytes);
        assert!(restored.renditions.is_empty());
        assert_eq!(
            restored.placeholder,
            placeholder::from_image(&source).unwrap()
        );
    }

    #[test]
    fn invalid_optional_rendition_does_not_discard_a_valid_master() {
        let source = DynamicImage::ImageRgba8(ImageBuffer::from_fn(640, 320, |x, y| {
            Rgba([(x % 251) as u8, (y % 239) as u8, ((x + y) % 241) as u8, 255])
        }));
        let asset = prepare_asset(
            &candidate("https://example.com/article.png"),
            png(&source),
            &MediaLimits::default(),
        )
        .unwrap();
        let mut metadata = asset.metadata("article");
        assert!(!metadata.variants.is_empty());
        let first = &metadata.variants[0];
        let unrelated =
            ImageBuffer::from_pixel(first.width, first.height, Rgba([250, 10, 80, 255]));
        let unrelated = lossless_webp(&unrelated, MediaLimits::default().max_file_bytes).unwrap();
        metadata.variants[0].file = format!(
            "article.image-{}.webp",
            &crate::model::sha1_hex(&unrelated.bytes)[..12]
        );
        let mut variants = asset
            .renditions
            .iter()
            .map(|rendition| rendition.bytes.clone())
            .collect::<Vec<_>>();
        variants[0] = unrelated.bytes.clone();

        let restored = Asset::from_stored(&metadata, asset.master_bytes.clone(), variants).unwrap();

        assert_eq!(restored.master_bytes, asset.master_bytes);
        assert!(
            restored
                .renditions
                .iter()
                .all(|rendition| rendition.bytes != unrelated.bytes)
        );
    }

    #[test]
    fn webp_master_is_not_duplicated_as_a_full_width_rendition() {
        let source = ImageBuffer::from_fn(800, 480, |x, y| {
            let mut value = u64::from(y) * 800 + u64::from(x) + 0x9e37_79b9_7f4a_7c15;
            value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            value ^= value >> 31;
            Rgba([value as u8, (value >> 8) as u8, (value >> 16) as u8, 255])
        });
        let bytes = lossless_webp(&source, MediaLimits::default().max_file_bytes)
            .unwrap()
            .bytes;
        let asset = prepare_asset(
            &candidate("https://example.com/source.webp"),
            bytes.clone(),
            &MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(asset.master_bytes, bytes);
        assert_eq!(asset.master_extension, "webp");
        assert!(!asset.renditions.is_empty());
        assert!(
            asset
                .renditions
                .iter()
                .all(|rendition| rendition.width < asset.width)
        );
        let restored = Asset::from_stored(
            &asset.metadata("source"),
            asset.master_bytes.clone(),
            asset
                .renditions
                .iter()
                .map(|rendition| rendition.bytes.clone())
                .collect(),
        )
        .unwrap();
        assert_eq!(restored.master_bytes, bytes);
        assert!(
            restored
                .renditions
                .iter()
                .all(|rendition| rendition.width < restored.width)
        );
    }

    #[test]
    fn animation_is_preserved_as_an_exact_master_only() {
        let red = ImageBuffer::from_pixel(80, 48, Rgba([240, 10, 10, 255]));
        let green = ImageBuffer::from_pixel(80, 48, Rgba([10, 240, 10, 255]));
        let mut bytes = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut bytes)
            .encode_frames([image::Frame::new(red.clone()), image::Frame::new(green)])
            .unwrap();
        let asset = prepare_asset(
            &candidate("https://example.com/animation.gif"),
            bytes.clone(),
            &MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(asset.master_bytes, bytes);
        assert_eq!(asset.master_extension, "gif");
        assert!(asset.renditions.is_empty());
        assert_eq!(
            asset.placeholder,
            placeholder::from_image(&DynamicImage::ImageRgba8(red)).unwrap()
        );
        let restored = Asset::from_stored(
            &asset.metadata("animation"),
            asset.master_bytes.clone(),
            vec![],
        )
        .unwrap();
        assert_eq!(restored.master_bytes, bytes);
        assert!(restored.renditions.is_empty());
        assert_eq!(restored.placeholder, asset.placeholder);
    }

    #[test]
    fn animated_png_is_preserved_as_an_exact_master_only() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 32, 32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_animated(2, 0).unwrap();
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&vec![0x33; 32 * 32 * 4]).unwrap();
            writer.write_image_data(&vec![0xcc; 32 * 32 * 4]).unwrap();
            writer.finish().unwrap();
        }
        let asset = prepare_asset(
            &candidate("https://example.com/animation.png"),
            bytes.clone(),
            &MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(asset.master_bytes, bytes);
        assert_eq!(asset.master_extension, "png");
        assert!(asset.renditions.is_empty());
        let restored = Asset::from_stored(
            &asset.metadata("animation"),
            asset.master_bytes.clone(),
            vec![],
        )
        .unwrap();
        assert_eq!(restored.master_bytes, bytes);
        assert!(restored.renditions.is_empty());
    }

    #[test]
    fn high_bit_depth_input_stays_master_only() {
        let image = DynamicImage::ImageRgba16(ImageBuffer::from_pixel(
            80,
            48,
            image::Rgba([32_768_u16; 4]),
        ));
        let bytes = png(&image);
        let asset = prepare_asset(
            &candidate("https://example.com/deep.png"),
            bytes.clone(),
            &MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(asset.master_bytes, bytes);
        assert!(asset.renditions.is_empty());
    }

    #[test]
    fn icc_profile_input_stays_master_only() {
        let image = ImageBuffer::from_fn(800, 480, |x, y| {
            Rgba([(x % 251) as u8, (y % 239) as u8, ((x + y) % 241) as u8, 255])
        });
        let bytes = png_with_icc(&image);
        let asset = prepare_asset(
            &candidate("https://example.com/profiled.png"),
            bytes.clone(),
            &MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(asset.master_bytes, bytes);
        assert!(asset.renditions.is_empty());
    }

    #[test]
    fn small_images_are_archived_but_malformed_and_dimension_bombs_are_rejected() {
        let candidate = candidate("https://example.com/bad.png");
        assert!(
            prepare_asset(
                &candidate,
                b"not an image".to_vec(),
                &MediaLimits::default()
            )
            .is_err()
        );
        assert!(
            prepare_asset(
                &candidate,
                png(&DynamicImage::new_rgba8(1, 1)),
                &MediaLimits::default()
            )
            .is_ok()
        );
        let limits = MediaLimits {
            max_axis: 64,
            ..MediaLimits::default()
        };
        assert!(prepare_asset(&candidate, png(&DynamicImage::new_rgba8(65, 32)), &limits).is_err());
    }

    #[test]
    fn stored_validation_rejects_tampered_names_dimensions_and_roles() {
        let image = DynamicImage::new_rgba8(400, 240);
        let asset = prepare_asset(
            &candidate("https://example.com/image.png"),
            png(&image),
            &MediaLimits::default(),
        )
        .unwrap();
        let metadata = asset.metadata("article");
        assert!(
            validate_stored(&asset.master_bytes, &metadata.original, StoredKind::Master).is_ok()
        );
        assert!(
            validate_stored(
                &asset.master_bytes,
                &metadata.original,
                StoredKind::Rendition
            )
            .is_err()
        );
        let mut wrong_name = metadata.original.clone();
        wrong_name.file = "article.image-000000000000.png".into();
        assert!(validate_stored(&asset.master_bytes, &wrong_name, StoredKind::Master).is_err());
        let mut wrong_dimensions = metadata.original;
        wrong_dimensions.width += 1;
        assert!(
            validate_stored(&asset.master_bytes, &wrong_dimensions, StoredKind::Master).is_err()
        );
    }

    #[tokio::test]
    async fn alternative_images_stop_after_first_usable_response() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        let missing = server.mock(|when, then| {
            when.path("/missing");
            then.status(404);
        });
        let poster = server.mock(|when, then| {
            when.path("/poster.png");
            then.status(200)
                .body(png(&DynamicImage::new_rgba8(640, 360)));
        });
        let unused = server.mock(|when, then| {
            when.path("/unused.png");
            then.status(200).body(png(&DynamicImage::new_rgba8(80, 48)));
        });
        let config = crate::config::Config::parse(&format!(
            "[[sources]]\nurl = [{:?}]",
            server.url("/feed")
        ))
        .unwrap();
        let source = config.sources().unwrap().remove(0);
        let fetcher = Fetcher::new(
            &FetchConfig {
                retries: 0,
                ..FetchConfig::default()
            },
            MediaLimits::default(),
        )
        .unwrap();
        let candidates = [
            candidate(&server.url("/missing")),
            candidate(&server.url("/poster.png")),
            candidate(&server.url("/unused.png")),
        ];
        let asset = fetcher.fetch_first(&candidates, &source).await.unwrap();
        assert_eq!((asset.width, asset.height), (640, 360));
        missing.assert_calls(1);
        poster.assert_calls(1);
        unused.assert_calls(0);
    }

    #[test]
    fn archived_cdn_tokens_require_one_unambiguous_saved_original() {
        let base = Url::parse("https://www.techemails.com/p/story").unwrap();
        let broken = base.join("q_auto:good").unwrap();
        let first = "https://substackcdn.com/image/fetch/w_40,q_auto:good,fl_progressive:steep/https%3A%2F%2Fimages.example%2Favatar.png";
        let second = first.replace("w_40", "w_80");
        let html = format!(r#"<img srcset="{first} 1x, {second} 2x">"#);
        assert_eq!(
            archived_image_request_url(&broken, &html, &base)
                .unwrap()
                .as_str(),
            "https://images.example/avatar.png"
        );
        let ambiguous = format!("{html}{}", html.replace("avatar.png", "another.png"));
        assert!(archived_image_request_url(&broken, &ambiguous, &base).is_none());
        assert!(archived_image_request_url(&broken, "<img src='avatar.png'>", &base).is_none());
        assert!(
            archived_image_request_url(&base.join("unrelated.png").unwrap(), &html, &base)
                .is_none()
        );
        let unsafe_html = html.replace(
            "https%3A%2F%2Fimages.example%2Favatar.png",
            "javascript%3Aalert(1)",
        );
        assert!(archived_image_request_url(&broken, &unsafe_html, &base).is_none());
    }

    #[test]
    fn recovered_cdn_tail_accepts_only_safe_absolute_origins() {
        let recover = |tail: &str| {
            image_request_url(
                &Url::parse(&format!(
                    "https://publisher.example/p/fl_progressive:steep/{tail}"
                ))
                .unwrap(),
            )
        };
        assert_eq!(
            recover("https%3A%2F%2Fimages.example%2Ffigure.png")
                .unwrap()
                .as_str(),
            "https://images.example/figure.png"
        );
        for tail in [
            "javascript%3Aalert(1)",
            "data%3Aimage/png;base64,AAAA",
            "https%3A%2F%2Fuser%3Asecret%40images.example%2Ffigure.png",
            "%EF%FF",
            "%ZZ",
            "relative.png",
        ] {
            assert!(recover(tail).is_none(), "{tail}");
        }
        let intact = Url::parse("https://substackcdn.com/image/fetch/w_640,c_limit,fl_progressive:steep/https%3A%2F%2Fimages.example%2Ffigure.png").unwrap();
        assert_eq!(image_request_url(&intact), Some(intact));
    }

    #[tokio::test]
    async fn saved_srcset_repairs_a_broken_alias_without_forwarding_source_credentials() {
        use httpmock::prelude::*;
        let publisher = MockServer::start();
        let origin = MockServer::start();
        let bytes = png(&DynamicImage::new_rgba8(80, 48));
        let image = origin.mock(|when, then| {
            when.path("/avatar.png").header_missing("authorization");
            then.status(200).body(bytes.clone());
        });
        let broken = publisher.mock(|when, then| {
            when.path("/p/q_auto:good");
            then.status(404);
        });
        let unavailable = origin.mock(|when, then| {
            when.path("/missing.png");
            then.status(404);
        });
        let encoded: String = origin
            .url("/avatar.png")
            .bytes()
            .map(|byte| format!("%{byte:02X}"))
            .collect();
        let full = format!(
            "https://substackcdn.com/image/fetch/w_120,q_auto:good,fl_progressive:steep/{encoded}"
        );
        let html = format!(r#"<img srcset="{full} 3x">"#);
        let base = Url::parse(&publisher.url("/p/story")).unwrap();
        let alias = publisher.url("/p/q_auto:good");
        let config = crate::config::Config::parse(&format!(
            "[[sources]]\nurl = {:?}\nheaders = {{ Authorization = \"Bearer private\" }}\n",
            publisher.url("/feed")
        ))
        .unwrap();
        let source = config.sources().unwrap().remove(0);
        let cache = tempfile::tempdir().unwrap();
        let fetcher = Fetcher::new(&FetchConfig::default(), MediaLimits::default())
            .unwrap()
            .with_cache(cache.path());
        assert!(
            fetcher
                .fetch(&[candidate(&alias)], &source)
                .await
                .is_empty()
        );
        let missing: String = origin
            .url("/missing.png")
            .bytes()
            .map(|byte| format!("%{byte:02X}"))
            .collect();
        let missing_html = html.replace(&encoded, &missing);
        for _ in 0..2 {
            assert!(
                fetcher
                    .fetch_archived_with_budget(
                        &[candidate(&alias)],
                        &source,
                        MediaLimits::default().max_article_bytes,
                        &missing_html,
                        &base,
                    )
                    .await
                    .is_empty()
            );
        }
        let assets = fetcher
            .fetch_archived_with_budget(
                &[candidate(&alias)],
                &source,
                MediaLimits::default().max_article_bytes,
                &html,
                &base,
            )
            .await;
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].source_url, alias);
        assert_eq!(assets[0].metadata("story").source, alias);
        assert_eq!(assets[0].master_bytes, bytes);
        image.assert_calls(1);
        broken.assert_calls(1);
        unavailable.assert_calls(1);
    }

    #[tokio::test]
    async fn recovered_cdn_tail_keeps_archived_identity_and_downloads_origin_once() {
        use httpmock::prelude::*;
        let publisher = MockServer::start();
        let origin = MockServer::start();
        let bytes = png(&DynamicImage::new_rgba8(80, 48));
        let image = origin.mock(|when, then| {
            when.path("/figure.png").header_missing("authorization");
            then.status(200).body(bytes.clone());
        });
        let encoded: String = origin
            .url("/figure.png")
            .bytes()
            .map(|byte| format!("%{byte:02X}"))
            .collect();
        let alias = publisher.url(format!("/p/fl_progressive:steep/{encoded}"));
        let config = crate::config::Config::parse(&format!(
            "[[sources]]\nurl = {:?}\nheaders = {{ Authorization = \"Bearer private\" }}\n",
            publisher.url("/feed")
        ))
        .unwrap();
        let source = config.sources().unwrap().remove(0);
        let assets = Fetcher::new(&FetchConfig::default(), MediaLimits::default())
            .unwrap()
            .fetch(
                &[
                    candidate(&alias),
                    candidate(&alias),
                    candidate(&origin.url("/figure.png")),
                ],
                &source,
            )
            .await;
        assert_eq!(assets.len(), 2);
        assert_eq!(assets[0].source_url, alias);
        assert_eq!(
            assets[0].source_hash,
            crate::model::sha1_hex(alias.as_bytes())
        );
        assert_eq!(assets[0].metadata("article").source, alias);
        assert!(assets[0].metadata("article").is_valid_for("article"));
        assert_eq!(assets[0].master_bytes, bytes);
        assert_eq!(assets[1].source_url, origin.url("/figure.png"));
        assert_eq!(assets[0].master_hash, assets[1].master_hash);
        image.assert_calls(1);
    }

    #[tokio::test]
    async fn fetch_is_failure_isolated_ordered_and_origin_scoped() {
        use httpmock::prelude::*;

        let source_server = MockServer::start();
        let other_server = MockServer::start();
        let broken = source_server.mock(|when, then| {
            when.path("/broken")
                .header("authorization", "Bearer private");
            then.status(200).body("not an image");
        });
        let local = source_server.mock(|when, then| {
            when.path("/local.png")
                .header("authorization", "Bearer private");
            then.status(200).body(png(&DynamicImage::new_rgba8(80, 48)));
        });
        let remote = other_server.mock(|when, then| {
            when.path("/remote.png").header_missing("authorization");
            then.status(200).body(png(&DynamicImage::new_rgba8(96, 64)));
        });
        let config = crate::config::Config::parse(&format!(
            "[[sources]]\nurl = {:?}\nheaders = {{ Authorization = \"Bearer private\" }}\n",
            source_server.url("/feed")
        ))
        .unwrap();
        let source = config.sources().unwrap().remove(0);
        let candidates = [
            candidate(&source_server.url("/broken")),
            candidate(&source_server.url("/local.png")),
            candidate(&other_server.url("/remote.png")),
        ];
        let assets = Fetcher::new(&FetchConfig::default(), MediaLimits::default())
            .unwrap()
            .fetch(&candidates, &source)
            .await;
        assert_eq!(
            assets
                .iter()
                .map(|asset| asset.source_url.as_str())
                .collect::<Vec<_>>(),
            [
                source_server.url("/local.png"),
                other_server.url("/remote.png")
            ]
        );
        broken.assert_calls(1);
        local.assert_calls(1);
        remote.assert_calls(1);
    }

    #[tokio::test]
    async fn retained_budget_skips_an_oversized_master_and_keeps_later_images() {
        use httpmock::prelude::*;

        let noisy = |width: u32, height: u32, salt: u64| {
            DynamicImage::ImageRgba8(ImageBuffer::from_fn(width, height, |x, y| {
                let mut value = u64::from(y) * u64::from(width) + u64::from(x) + salt;
                value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
                value ^= value >> 31;
                Rgba([value as u8, (value >> 8) as u8, (value >> 16) as u8, 255])
            }))
        };
        let first_bytes = png(&noisy(400, 240, 1));
        let skipped_bytes = png(&noisy(800, 800, 2));
        let final_bytes = png(&DynamicImage::new_rgba8(32, 32));
        let max_file_bytes = skipped_bytes
            .len()
            .max(first_bytes.len())
            .max(final_bytes.len());
        let limits = MediaLimits {
            max_file_bytes,
            max_article_bytes: first_bytes.len() + skipped_bytes.len() + final_bytes.len(),
            ..MediaLimits::default()
        };
        let prepared = prepare_asset(
            &candidate("https://example.com/first.png"),
            first_bytes.clone(),
            &limits,
        )
        .unwrap();
        let rendition_bytes = prepared
            .renditions
            .iter()
            .map(|rendition| rendition.bytes.len())
            .sum::<usize>();
        assert!(rendition_bytes > final_bytes.len());
        assert!(skipped_bytes.len() > rendition_bytes);

        let server = MockServer::start_async().await;
        for (path, body) in [
            ("/first.png", first_bytes),
            ("/skipped.png", skipped_bytes),
            ("/final.png", final_bytes),
        ] {
            server
                .mock_async(move |when, then| {
                    when.path(path);
                    then.status(200).body(body);
                })
                .await;
        }
        let config = crate::config::Config::parse(&format!(
            "[[sources]]\nurl = {:?}\n",
            server.url("/feed")
        ))
        .unwrap();
        let source = config.sources().unwrap().remove(0);
        let candidates = ["first.png", "skipped.png", "final.png"]
            .map(|path| candidate(&server.url(format!("/{path}"))));

        let assets = Fetcher::new(&FetchConfig::default(), limits)
            .unwrap()
            .fetch(&candidates, &source)
            .await;

        assert_eq!(
            assets
                .iter()
                .map(|asset| asset.source_url.as_str())
                .collect::<Vec<_>>(),
            [server.url("/first.png"), server.url("/final.png")]
        );
    }

    #[test]
    fn budget_pruning_never_retains_an_unpublished_partial_srcset() {
        let mut asset = Asset {
            source_url: "https://example.com/image.png".into(),
            source_hash: "source".into(),
            alt: None,
            master_bytes: vec![0; 6],
            master_extension: "png",
            master_hash: "master".into(),
            width: 800,
            height: 400,
            dominant_color: "#000000".into(),
            placeholder: placeholder::from_image(&DynamicImage::new_rgb8(1, 1)).unwrap(),
            renditions: [(48, 24, 2), (320, 160, 2), (800, 400, 2)]
                .into_iter()
                .map(|(width, height, bytes)| Rendition {
                    bytes: vec![0; bytes],
                    extension: "webp",
                    hash: format!("{width}"),
                    width,
                    height,
                })
                .collect(),
        };

        let used = fit_asset_to_budget(&mut asset, 10).unwrap();

        assert_eq!(used, 6);
        assert!(asset.renditions.is_empty());
    }

    #[tokio::test]
    async fn download_limit_bounds_responses_before_they_wait_for_decoding() {
        use httpmock::prelude::*;

        let server = MockServer::start_async().await;
        let image = png(&DynamicImage::new_rgba8(80, 48));
        for path in ["/one.png", "/two.png"] {
            server
                .mock_async(|when, then| {
                    when.path(path);
                    then.status(200)
                        .delay(Duration::from_millis(100))
                        .body(image.clone());
                })
                .await;
        }
        let config = crate::config::Config::parse(&format!(
            "[[sources]]\nurl = {:?}\n",
            server.url("/feed")
        ))
        .unwrap();
        let source = config.sources().unwrap().remove(0);
        let fetcher = Fetcher::new(
            &FetchConfig::default(),
            MediaLimits {
                download_concurrency: 1,
                decode_concurrency: 1,
                ..MediaLimits::default()
            },
        )
        .unwrap();
        let one = [candidate(&server.url("/one.png"))];
        let two = [candidate(&server.url("/two.png"))];

        let started = std::time::Instant::now();
        let (one, two) = tokio::join!(fetcher.fetch(&one, &source), fetcher.fetch(&two, &source));

        assert_eq!((one.len(), two.len()), (1, 1));
        assert!(started.elapsed() >= Duration::from_millis(180));
    }
}
