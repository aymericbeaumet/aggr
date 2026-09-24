//! Bounded local copies of article-body images.
//!
//! Under `images = "original"` the publisher's bytes remain the master, and responsive renditions
//! are resized once from the oriented decode and encoded as lossless WebP, then decoded again to
//! prove pixel equality before use. Under `images = "compact"` one bounded, lossy master replaces
//! both; see [`compact`]. `images = "remote"` never reaches this module.

pub mod compact;
pub mod placeholder;
pub(crate) mod srcset;
mod stored_cache;
mod vector;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use image::{
    AnimationDecoder as _, DynamicImage, ExtendedColorType, GenericImageView as _,
    ImageDecoder as _, ImageFormat, ImageReader,
};
use scraper::{Html, Selector};
use sha2::{Digest as _, Sha256};
use tokio::sync::Semaphore;
use url::Url;

use crate::config::{FetchConfig, Source};
use crate::http;
use crate::model::{ArticleImage, ImageFile};

pub use compact::CompactPolicy;
pub(crate) use stored_cache::StoredAssetCache;

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

/// Status badges remain readable body images, but are not article artwork. Badge services name
/// themselves: the host or a path segment carries `badge`, `badgen` or `shields`. An image proxy
/// that hex-encodes the origin in its last path segment is unwrapped one level.
pub fn is_status_badge(source: &str) -> bool {
    fn labelled(value: &str) -> bool {
        value
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .any(|token| matches!(token, "badge" | "badges" | "badgen" | "shield" | "shields"))
    }
    fn matches(source: &str, allow_proxy: bool) -> bool {
        let Ok(url) = Url::parse(source) else {
            return false;
        };
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        let path = url.path().to_ascii_lowercase();
        if labelled(&host) || path.split('/').any(labelled) {
            return true;
        }
        allow_proxy
            && path
                .rsplit('/')
                .next()
                .filter(|value| value.len() <= 8192 && value.len() % 2 == 0)
                .and_then(|value| hex::decode(value).ok())
                .and_then(|value| String::from_utf8(value).ok())
                .is_some_and(|value| value.starts_with("http") && matches(&value, false))
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
    /// Wall-clock budget for one article's image candidates. Candidates are still fetched one
    /// at a time; once the budget is spent the remaining candidates are skipped and the images
    /// already retained are kept, so one slow CDN cannot occupy its source for many minutes.
    pub article_timeout: Duration,
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
            article_timeout: Duration::from_secs(120),
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

/// Directory name of the image-failure markers written by this media implementation; an older
/// generation's markers are stale and may be swept.
pub(crate) fn image_failure_generation() -> &'static str {
    static GENERATION: OnceLock<String> = OnceLock::new();
    GENERATION.get_or_init(|| hex::encode(implementation_fingerprint()))
}

fn implementation_fingerprint() -> &'static [u8; 32] {
    static IMPLEMENTATION: OnceLock<[u8; 32]> = OnceLock::new();
    IMPLEMENTATION.get_or_init(|| {
        let mut hash = Sha256::new();
        hash.update(include_bytes!("media.rs"));
        hash.update(include_bytes!("media/placeholder.rs"));
        hash.update(include_bytes!("media/srcset.rs"));
        hash.update(include_bytes!("media/stored_cache.rs"));
        hash.update(include_bytes!("media/vector.rs"));
        hash.update(include_bytes!(
            "media/fonts/AtkinsonHyperlegible-Regular.ttf"
        ));
        hash.update(include_bytes!("../Cargo.lock"));
        hash.finalize().into()
    })
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
            alt: candidate.alt.as_deref().and_then(crate::model::image_alt),
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
                alt: image.value().attr("alt").and_then(crate::model::image_alt),
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
    // Bare video URLs autolink in the rendered reader, so recognize them here as well.
    let mut options = comrak::Options::default();
    options.extension.autolink = true;
    let root = comrak::parse_document(&arena, markdown, &options);
    let mut candidates = Vec::new();
    for node in root.descendants() {
        let data = node.data.borrow();
        match &data.value {
            comrak::nodes::NodeValue::Image(link) => {
                if let Some(url) = safe_image_url(&link.url, base) {
                    candidates.push(Candidate { url, alt: None });
                }
            }
            // A paragraph that is only a video link renders as an inline player; archive its
            // poster so the facade never requests the provider before activation.
            comrak::nodes::NodeValue::Paragraph => {
                if let Some(url) = standalone_video_link(node)
                    .and_then(|link| base.join(&link).ok())
                    .and_then(|url| crate::sources::youtube::thumbnail(&url))
                    .and_then(|thumbnail| Url::parse(&thumbnail).ok())
                {
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

/// The URL of a paragraph consisting of one link (text or one image inside), else `None`.
pub(crate) fn standalone_video_link<'a>(
    paragraph: &'a comrak::nodes::AstNode<'a>,
) -> Option<String> {
    use comrak::nodes::NodeValue;
    let mut link = None;
    for child in paragraph.children() {
        match &child.data.borrow().value {
            NodeValue::Link(target) if link.is_none() => link = Some(target.url.clone()),
            NodeValue::Text(text) if text.trim().is_empty() => {}
            NodeValue::SoftBreak | NodeValue::LineBreak => {}
            _ => return None,
        }
    }
    link
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
                // The `/image/fetch/<transforms>/<encoded origin>` layout of fetch-style CDNs.
                if !full.path().contains("/image/fetch/")
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
        ensure!(
            limits.article_timeout >= Duration::from_secs(fetch.timeout_secs),
            "media article timeout must be at least the request timeout"
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
        self.failure_cache = Some(
            crate::cache::Namespace::ImageFailures
                .dir(directory)
                .join(image_failure_generation()),
        );
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
        let started = Instant::now();
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
            // Checked between candidates only: an in-flight request keeps its own timeout and
            // its outcome, so a slow but successful image is never marked as failed.
            if started.elapsed() >= self.limits.article_timeout {
                log::debug!(
                    "{}: image budget of {:?} spent after {:?}: keeping {} images, skipping the \
                     remaining candidates from {}",
                    source.slug,
                    self.limits.article_timeout,
                    started.elapsed(),
                    assets.len(),
                    candidate.url
                );
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
            let compact = source.images.compaction();
            let task = tokio::task::spawn_blocking(move || {
                // A timed-out blocking task cannot be cancelled. Keep its shared slot until its
                // decoder really exits so subsequent articles remain bounded.
                let _permit = permit;
                prepare_asset_with_policy(&decoding_candidate, body.bytes, &limits, compact)
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

/// Validate and decode one response, keeping raster bytes (or a passive SVG raster) as the master
/// and deriving only pixel-lossless WebP renditions that fit the configured limits. Every archive
/// path carries a source's policy, so this full-fidelity shorthand belongs to tests.
#[cfg(test)]
pub fn prepare_asset(candidate: &Candidate, bytes: Vec<u8>, limits: &MediaLimits) -> Result<Asset> {
    prepare_asset_with_policy(candidate, bytes, limits, None)
}

/// Archive one response, reducing the master to `compact` when a source asks for a compact
/// archive. Compaction needs the same intact 8-bit decode that renditions do, so an animated,
/// colour-managed or high-depth image keeps its exact bytes under either policy.
pub fn prepare_asset_with_policy(
    candidate: &Candidate,
    bytes: Vec<u8>,
    limits: &MediaLimits,
    compact: Option<CompactPolicy>,
) -> Result<Asset> {
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
    if format == ImageFormat::Avif {
        return avif_asset(candidate, bytes, limits);
    }
    let extension = match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Png => "png",
        ImageFormat::Gif => "gif",
        ImageFormat::WebP => "webp",
        // Every raster format that decodes without a system library: a publisher's choice of
        // container is not a reason to lose the picture. The renditions below are WebP either way.
        ImageFormat::Bmp => "bmp",
        ImageFormat::Ico => "ico",
        ImageFormat::Tiff => "tiff",
        ImageFormat::Qoi => "qoi",
        _ => bail!("unsupported article image format: {format:?}"),
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

    // Both policies need the same intact decode; a compact master simply replaces the renditions
    // it would otherwise have derived, so nothing is encoded twice. An animation has no still to
    // replace and is reduced as an animation instead.
    let reducible = !animated && !has_icc && safe_for_renditions(original_color);
    let compact_animation = compact.is_some() && animated && !has_icc && format == ImageFormat::Gif;
    let compact = compact.filter(|_| reducible || compact_animation);

    let rendition_widths = limits
        .rendition_widths
        .iter()
        .copied()
        .filter(|width| *width > 0 && *width < image.width())
        .collect::<BTreeSet<_>>();
    let mut renditions = Vec::with_capacity(rendition_widths.len() + 1);
    if reducible && compact.is_none() {
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
    let mut asset = Asset {
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
    };
    if let Some(policy) = compact {
        if compact_animation {
            compact::apply_animation(&mut asset, policy, limits);
        } else {
            compact::apply(&mut asset, &image, policy)?;
        }
    }
    Ok(asset)
}

/// AVIF needs an AV1 decoder, which is a system library rather than a crate, and the picture is
/// worth more than what decoding it would add. The response is archived exactly as it arrived and
/// served as the master: every browser that a publisher chose AVIF for can read it. Its geometry
/// comes from the container so the space is still reserved before it loads; what is lost is the
/// derived WebP renditions and the inline preview, and the dominant colour stands in for those.
fn avif_asset(candidate: &Candidate, bytes: Vec<u8>, limits: &MediaLimits) -> Result<Asset> {
    let (width, height) = avif_dimensions(&bytes).context("reading AVIF image size")?;
    validate_dimensions(width, height, limits)?;
    // A flat stand-in keeps every promise the reader depends on: a valid inline preview that waits
    // for no request, and a colour holding the space until the picture itself arrives. Only the
    // likeness is missing, and the picture behind it is the publisher's own bytes.
    let stand_in = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        4,
        4,
        image::Rgb([122, 122, 126]),
    ));
    Ok(Asset {
        source_url: candidate.url.to_string(),
        source_hash: crate::model::sha1_hex(candidate.url.as_str().as_bytes()),
        alt: candidate.alt.clone(),
        master_hash: crate::model::sha1_hex(&bytes),
        master_bytes: bytes,
        master_extension: "avif",
        width,
        height,
        dominant_color: dominant_color(&stand_in),
        placeholder: placeholder::from_image(&stand_in)?,
        renditions: Vec::new(),
    })
}

/// The `ispe` box states an AVIF's stored size in its first full-box payload: version and flags,
/// then two big-endian `u32`s. Walking the ISO base media boxes to it reads the size without
/// decoding a single pixel.
fn avif_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    fn walk(mut data: &[u8], depth: usize) -> Option<(u32, u32)> {
        if depth > 8 {
            return None;
        }
        while data.len() >= 8 {
            let size = u32::from_be_bytes(data[0..4].try_into().ok()?) as usize;
            let kind = &data[4..8];
            // `0` runs to the end of the file and `1` carries a 64-bit size aggr does not need.
            let size = if size == 0 { data.len() } else { size };
            if size < 8 || size > data.len() {
                return None;
            }
            let payload = &data[8..size];
            if kind == b"ispe" && payload.len() >= 12 {
                let width = u32::from_be_bytes(payload[4..8].try_into().ok()?);
                let height = u32::from_be_bytes(payload[8..12].try_into().ok()?);
                return (width > 0 && height > 0).then_some((width, height));
            }
            // Only the containers on the way to `ispe`, so no payload is mistaken for boxes.
            if matches!(kind, b"meta" | b"iprp" | b"ipco") {
                // `meta` is a full box: its version and flags come before its children.
                let children = if kind == b"meta" { 4 } else { 0 };
                if let Some(found) = payload
                    .get(children..)
                    .and_then(|rest| walk(rest, depth + 1))
                {
                    return Some(found);
                }
            }
            data = &data[size..];
        }
        None
    }
    walk(bytes, 0)
}

#[cfg(test)]
mod avif_tests {
    use super::*;

    fn box_bytes(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    /// `ftyp`, then `meta` as a full box holding `iprp` → `ipco` → `ispe`, which is where an AVIF
    /// states the size aggr reserves space for.
    fn avif(width: u32, height: u32) -> Vec<u8> {
        let mut ispe = vec![0, 0, 0, 0];
        ispe.extend_from_slice(&width.to_be_bytes());
        ispe.extend_from_slice(&height.to_be_bytes());
        let ipco = box_bytes(b"ipco", &box_bytes(b"ispe", &ispe));
        let iprp = box_bytes(b"iprp", &ipco);
        let mut meta = vec![0, 0, 0, 0];
        meta.extend_from_slice(&iprp);
        let mut out = box_bytes(b"ftyp", b"avif\0\0\0\0avifmif1");
        out.extend_from_slice(&box_bytes(b"meta", &meta));
        out
    }

    #[test]
    fn an_avif_states_its_size_where_a_decoder_is_not_needed_to_read_it() {
        assert_eq!(avif_dimensions(&avif(3904, 2574)), Some((3904, 2574)));
        // Nothing to read: no `ispe`, a zero extent, and a box claiming more than it has.
        assert_eq!(avif_dimensions(&box_bytes(b"ftyp", b"avif")), None);
        assert_eq!(avif_dimensions(&avif(0, 2574)), None);
        let mut truncated = avif(100, 100);
        truncated[0..4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(avif_dimensions(&truncated), None);
    }

    #[test]
    fn an_avif_is_archived_whole_rather_than_dropped() {
        let bytes = avif(1200, 800);
        let candidate = Candidate {
            url: Url::parse("https://example.com/picture.avif").unwrap(),
            alt: Some("A picture".into()),
        };
        let asset = avif_asset(&candidate, bytes.clone(), &MediaLimits::default()).unwrap();
        assert_eq!(asset.master_bytes, bytes, "the publisher's own bytes");
        assert_eq!(asset.master_extension, "avif");
        assert_eq!((asset.width, asset.height), (1200, 800));
        assert!(asset.renditions.is_empty(), "no decoder, no renditions");
        assert!(
            asset
                .placeholder
                .data_url
                .starts_with("data:image/png;base64,"),
            "the space is still held without a request: {}",
            asset.placeholder.data_url
        );
    }
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

    pub(super) fn png(image: &DynamicImage) -> Vec<u8> {
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

    pub(super) fn candidate(url: &str) -> Candidate {
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
    fn standalone_video_links_archive_their_poster() {
        let base = Url::parse("https://blog.example/post").unwrap();
        let markdown = "Intro with https://youtu.be/inline1234 mentioned.\n\n[![](https://blog.example/thumb.png)](https://youtu.be/abcDEF12345)\n\nhttps://www.youtube.com/watch?v=xyz987_-ABC\n\n[Watch](https://youtu.be/short12345) and [more](https://example.com)\n";
        let urls: Vec<_> = markdown_candidates(markdown, &base)
            .into_iter()
            .map(|candidate| candidate.url.to_string())
            .collect();
        assert_eq!(
            urls,
            [
                "https://i.ytimg.com/vi/abcDEF12345/hqdefault.jpg",
                "https://blog.example/thumb.png",
                "https://i.ytimg.com/vi/xyz987_-ABC/hqdefault.jpg",
            ]
        );
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
            "https://example.com/logo.svg",
            "https://github.com/user/project/raw/main/diagram.svg",
            "https://example.com/gallery/badgers-in-spring.jpg",
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
    fn a_compact_archive_stores_one_bounded_master_and_no_renditions() {
        let source = DynamicImage::ImageRgba8(ImageBuffer::from_fn(2400, 1500, |x, y| {
            Rgba([(x % 251) as u8, (y % 239) as u8, ((x + y) % 241) as u8, 255])
        }));
        let bytes = png(&source);
        let asset = prepare_asset_with_policy(
            &candidate("https://example.com/photo.png"),
            bytes.clone(),
            &MediaLimits::default(),
            Some(CompactPolicy::archive()),
        )
        .unwrap();

        assert!(asset.renditions.is_empty());
        assert_eq!(asset.master_extension, "jpg");
        assert_eq!((asset.width, asset.height), (1600, 1000));
        assert!(
            asset.master_bytes.len() < bytes.len() / 4,
            "compact master kept {} of {} bytes",
            asset.master_bytes.len(),
            bytes.len()
        );

        // Every derived value describes the bytes that were stored, not the ones that arrived.
        assert_eq!(
            asset.master_hash,
            crate::model::sha1_hex(&asset.master_bytes)
        );
        assert_eq!(
            asset.placeholder,
            placeholder::from_image(&image::load_from_memory(&asset.master_bytes).unwrap())
                .unwrap()
        );
        let metadata = asset.metadata("article");
        assert!(metadata.is_valid_for("article") && metadata.variants.is_empty());
        assert_eq!(asset.files("article").len(), 1);
        assert_eq!(
            validate_stored(&asset.master_bytes, &metadata.original, StoredKind::Master).unwrap(),
            "jpg"
        );
    }

    #[test]
    fn a_compact_master_keeps_transparency_in_a_png() {
        let source = DynamicImage::ImageRgba8(ImageBuffer::from_fn(2000, 1200, |x, y| {
            Rgba([
                (x % 251) as u8,
                (y % 239) as u8,
                40,
                if (x + y) % 7 == 0 { 0 } else { 255 },
            ])
        }));
        let bytes = png(&source);
        let asset = prepare_asset_with_policy(
            &candidate("https://example.com/diagram.png"),
            bytes.clone(),
            &MediaLimits::default(),
            Some(CompactPolicy::archive()),
        )
        .unwrap();

        assert_eq!(asset.master_extension, "png");
        assert_eq!((asset.width, asset.height), (1600, 960));
        assert!(asset.master_bytes.len() < bytes.len());
        assert!(asset.renditions.is_empty());
        let decoded = image::load_from_memory(&asset.master_bytes).unwrap();
        assert!(
            decoded.to_rgba8().pixels().any(|pixel| pixel.0[3] != 255),
            "a compact copy dropped the transparency it was asked to keep"
        );
    }

    #[test]
    fn a_compact_animation_is_resized_and_still_animates() {
        // A small policy keeps the quantizer's work proportionate; the path it exercises is the
        // same one `archive()` takes on a publisher's animation.
        let policy = CompactPolicy {
            max_axis: 320,
            jpeg_quality: 72,
        };
        let frame = |shift: u32| {
            ImageBuffer::from_fn(900, 500, move |x, y| {
                Rgba([
                    ((x + shift) % 251) as u8,
                    (y % 239) as u8,
                    ((x + y) % 241) as u8,
                    255,
                ])
            })
        };
        let mut animation = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut animation)
            .encode_frames([
                image::Frame::new(frame(0)),
                image::Frame::new(frame(97)),
                image::Frame::new(frame(194)),
            ])
            .unwrap();

        let asset = prepare_asset_with_policy(
            &candidate("https://example.com/animation.gif"),
            animation.clone(),
            &MediaLimits::default(),
            Some(policy),
        )
        .unwrap();

        assert_eq!(asset.master_extension, "gif");
        assert!(asset.renditions.is_empty());
        assert_eq!(asset.width, policy.max_axis);
        assert!(
            asset.master_bytes.len() < animation.len(),
            "compact animation kept {} of {} bytes",
            asset.master_bytes.len(),
            animation.len()
        );

        // It has to still be the animation it replaced, not its first frame.
        let decoder =
            image::codecs::gif::GifDecoder::new(Cursor::new(asset.master_bytes.as_slice()))
                .unwrap();
        assert!(matches!(
            decoder.loop_count(),
            image::metadata::LoopCount::Infinite
        ));
        let frames = decoder.into_frames().collect_frames().unwrap();
        assert_eq!(frames.len(), 3);
        assert!(
            frames
                .iter()
                .all(|frame| frame.buffer().width() == asset.width)
        );
        assert_eq!(
            validate_stored(
                &asset.master_bytes,
                &asset.metadata("article").original,
                StoredKind::Master
            )
            .unwrap(),
            "gif"
        );
    }

    #[test]
    fn a_long_animation_stops_at_its_budget_instead_of_decoding_every_frame() {
        // Frames are held one at a time, but the work itself still has to end: a small, densely
        // compressed GIF can carry far more pixels than any single image is allowed to decode.
        let frame = |shade: u8| ImageBuffer::from_pixel(240, 160, Rgba([shade, 40, 90, 255]));
        let mut animation = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut animation)
            .encode_frames([
                image::Frame::new(frame(10)),
                image::Frame::new(frame(200)),
                image::Frame::new(frame(120)),
            ])
            .unwrap();

        // Enough to decode one 240x160 frame, not enough for all three.
        let limits = MediaLimits {
            max_pixels: 50_000,
            ..MediaLimits::default()
        };
        let asset = prepare_asset_with_policy(
            &candidate("https://example.com/long.gif"),
            animation.clone(),
            &limits,
            Some(CompactPolicy::archive()),
        )
        .unwrap();

        assert_eq!(asset.master_bytes, animation);
        assert_eq!(asset.master_extension, "gif");
        assert!(asset.renditions.is_empty());
    }

    #[test]
    fn an_animation_already_within_bounds_is_never_replaced_by_a_larger_one() {
        // Publishers' animations are usually frame-differenced already. Re-encoding one that does
        // not need resizing composites every frame again and can easily cost more than it saves,
        // so the exact original has to win that comparison.
        let frame = |shift: u32| {
            ImageBuffer::from_fn(240, 160, move |x, y| {
                Rgba([((x + shift) % 251) as u8, (y % 239) as u8, 80, 255])
            })
        };
        let mut animation = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut animation)
            .encode_frames([image::Frame::new(frame(0)), image::Frame::new(frame(120))])
            .unwrap();

        let asset = prepare_asset_with_policy(
            &candidate("https://example.com/small.gif"),
            animation.clone(),
            &MediaLimits::default(),
            Some(CompactPolicy::archive()),
        )
        .unwrap();

        assert_eq!(asset.master_extension, "gif");
        assert!(
            asset.master_bytes.len() <= animation.len(),
            "compaction grew an animation from {} to {} bytes",
            animation.len(),
            asset.master_bytes.len()
        );
        if asset.master_bytes.len() == animation.len() {
            assert_eq!(asset.master_bytes, animation);
        }
    }

    #[test]
    fn compaction_never_costs_an_image_what_it_cannot_re_encode() {
        // An APNG is an animation aggr cannot write, so it keeps the bytes it arrived with.
        let mut apng = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut apng, 900, 500);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_animated(2, 0).unwrap();
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&vec![0x33; 900 * 500 * 4]).unwrap();
            writer.write_image_data(&vec![0xcc; 900 * 500 * 4]).unwrap();
            writer.finish().unwrap();
        }
        let asset = prepare_asset_with_policy(
            &candidate("https://example.com/animated.png"),
            apng.clone(),
            &MediaLimits::default(),
            Some(CompactPolicy::archive()),
        )
        .unwrap();
        assert_eq!(asset.master_bytes, apng);
        assert_eq!(asset.master_extension, "png");
        assert_eq!((asset.width, asset.height), (900, 500));

        let managed = png_with_icc(&ImageBuffer::from_fn(1800, 1200, |x, y| {
            Rgba([(x % 251) as u8, (y % 239) as u8, 60, 255])
        }));
        let asset = prepare_asset_with_policy(
            &candidate("https://example.com/managed.png"),
            managed.clone(),
            &MediaLimits::default(),
            Some(CompactPolicy::archive()),
        )
        .unwrap();
        assert_eq!(asset.master_bytes, managed);
        assert_eq!(asset.master_extension, "png");
        assert_eq!((asset.width, asset.height), (1800, 1200));
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

    #[test]
    fn article_timeout_must_cover_one_request() {
        let fetch = FetchConfig {
            timeout_secs: 20,
            ..FetchConfig::default()
        };
        let limits = |article_timeout| MediaLimits {
            article_timeout,
            ..MediaLimits::default()
        };
        assert!(Fetcher::new(&fetch, limits(Duration::from_secs(19))).is_err());
        assert!(Fetcher::new(&fetch, limits(Duration::from_secs(20))).is_ok());
        assert!(Fetcher::new(&fetch, MediaLimits::default()).is_ok());
    }

    #[tokio::test]
    async fn article_timeout_stops_after_a_hanging_candidate_and_keeps_earlier_images() {
        use httpmock::prelude::*;

        let server = MockServer::start_async().await;
        let first = server
            .mock_async(|when, then| {
                when.path("/first.png");
                then.status(200).body(png(&DynamicImage::new_rgba8(80, 48)));
            })
            .await;
        let hanging = server
            .mock_async(|when, then| {
                when.path("/hanging.png");
                then.status(200)
                    .delay(Duration::from_secs(3))
                    .body(png(&DynamicImage::new_rgba8(64, 32)));
            })
            .await;
        let later = server
            .mock_async(|when, then| {
                when.path("/later.png");
                then.status(200).body(png(&DynamicImage::new_rgba8(48, 24)));
            })
            .await;
        let last = server
            .mock_async(|when, then| {
                when.path("/last.png");
                then.status(200).body(png(&DynamicImage::new_rgba8(40, 20)));
            })
            .await;
        let config = crate::config::Config::parse(&format!(
            "[[sources]]\nurl = {:?}\n",
            server.url("/feed")
        ))
        .unwrap();
        let source = config.sources().unwrap().remove(0);
        let fetcher = Fetcher::new(
            &FetchConfig {
                timeout_secs: 1,
                retries: 0,
                ..FetchConfig::default()
            },
            MediaLimits {
                article_timeout: Duration::from_secs(1),
                ..MediaLimits::default()
            },
        )
        .unwrap();
        let candidates = ["first.png", "hanging.png", "later.png", "last.png"]
            .map(|path| candidate(&server.url(format!("/{path}"))));

        let started = std::time::Instant::now();
        let assets = fetcher.fetch(&candidates, &source).await;

        assert_eq!(
            assets
                .iter()
                .map(|asset| asset.source_url.as_str())
                .collect::<Vec<_>>(),
            [server.url("/first.png")]
        );
        assert_eq!((assets[0].width, assets[0].height), (80, 48));
        assert!(started.elapsed() < Duration::from_secs(3));
        first.assert_calls(1);
        hanging.assert_calls(1);
        later.assert_calls(0);
        last.assert_calls(0);
    }
}
