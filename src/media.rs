//! Bounded, lossless local copies of article-body images.
//!
//! Original bytes remain the master. Responsive renditions are resized once from the oriented
//! decode and encoded as lossless WebP, then decoded again to prove pixel equality before use.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use image::{
    AnimationDecoder as _, DynamicImage, ExtendedColorType, GenericImageView as _,
    ImageDecoder as _, ImageFormat, ImageReader,
};
use scraper::{Html, Selector};
use tokio::sync::Semaphore;
use url::Url;

use crate::config::{FetchConfig, Source};
use crate::http;
use crate::model::{ArticleImage, ImageFile};

const MIN_AXIS: u32 = 32;
const PLACEHOLDER_WIDTH: u32 = 48;

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
            max_file_bytes: 10 * 1024 * 1024,
            max_article_bytes: 32 * 1024 * 1024,
            max_candidates: 24,
            max_assets: 12,
            max_pixels: 32_000_000,
            max_axis: 12_000,
            download_concurrency: 8,
            decode_concurrency: 2,
            decode_timeout: Duration::from_secs(15),
            rendition_widths: vec![PLACEHOLDER_WIDTH, 320, 640, 960, 1280, 1600],
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
    /// Exact bytes returned by the publisher, without metadata stripping or re-encoding.
    pub master_bytes: Vec<u8>,
    pub master_extension: &'static str,
    pub master_hash: String,
    /// Intrinsic dimensions after applying the master's orientation metadata.
    pub width: u32,
    pub height: u32,
    /// Deterministic opaque CSS color derived from the oriented image's visible pixels.
    pub dominant_color: String,
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
            metadata.variants.len() <= limits.rendition_widths.len().saturating_add(1),
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

        // Archives written before responsive companions were introduced still have a valid,
        // exact master. Derive only the tiny lossless placeholder at render/import time: it gives
        // those images progressive paint without changing their archived bytes or advertising a
        // partial responsive srcset that a large viewport would have to upscale.
        if renditions.is_empty()
            && !animated
            && !has_icc
            && safe_for_renditions(original_color)
            && master_image.width() > PLACEHOLDER_WIDTH
        {
            let width = PLACEHOLDER_WIDTH;
            let height = scaled_height(master_image.width(), master_image.height(), width);
            let resized = master_image
                .resize_exact(width, height, image::imageops::FilterType::Lanczos3)
                .to_rgba8();
            if let Ok(placeholder) = lossless_webp(&resized, limits.max_file_bytes)
                && placeholder.bytes.len() < master.len()
                && retained
                    .checked_add(placeholder.bytes.len())
                    .is_some_and(|total| total <= limits.max_article_bytes)
            {
                retained += placeholder.bytes.len();
                renditions.push(placeholder);
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
            renditions,
        })
    }
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
        .filter(|image| {
            !["width", "height"].into_iter().any(|attribute| {
                image
                    .value()
                    .attr(attribute)
                    .and_then(parse_dimension)
                    .is_some_and(|size| size < MIN_AXIS)
            })
        })
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

fn parse_dimension(value: &str) -> Option<u32> {
    value.trim().parse().ok()
}

fn is_data_url(value: &str) -> bool {
    value
        .trim_start_matches(|ch: char| ch.is_whitespace() || ch.is_control())
        .to_ascii_lowercase()
        .starts_with("data:")
}

fn safe_image_url(value: &str, base: &Url) -> Option<Url> {
    let mut url = base.join(value.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let path = url.path().to_ascii_lowercase();
    if path.ends_with(".svg") || path.ends_with(".svgz") {
        return None;
    }
    url.set_fragment(None);
    Some(url)
}

fn clean_alt(value: Option<&str>) -> Option<String> {
    value
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(300).collect())
}

pub struct Fetcher {
    client: http::Client,
    download_limit: Arc<Semaphore>,
    decode_limit: Arc<Semaphore>,
    limits: MediaLimits,
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
        })
    }

    /// Fetch candidates in source order. An unavailable or malformed image never prevents later
    /// candidates from succeeding; returned assets retain that deterministic source order.
    pub async fn fetch(&self, candidates: &[Candidate], source: &Source) -> Vec<Asset> {
        self.fetch_up_to(candidates, source, self.limits.max_assets)
            .await
    }

    /// Alternative URLs for one image stop after the first usable response.
    pub async fn fetch_first(&self, candidates: &[Candidate], source: &Source) -> Option<Asset> {
        self.fetch_up_to(candidates, source, 1).await.pop()
    }

    async fn fetch_up_to(
        &self,
        candidates: &[Candidate],
        source: &Source,
        max_assets: usize,
    ) -> Vec<Asset> {
        let mut assets = Vec::new();
        let mut downloaded = 0_usize;
        let mut retained = 0_usize;
        for candidate in candidates.iter().take(self.limits.max_candidates) {
            if assets.len() >= max_assets || downloaded >= self.limits.max_article_bytes {
                break;
            }
            let Ok(download_permit) = self.download_limit.clone().acquire_owned().await else {
                break;
            };
            let response = self
                .client
                .get(http::Request {
                    url: &candidate.url,
                    headers: http::source_headers(source, &candidate.url),
                    etag: None,
                    last_modified: None,
                })
                .await;
            let Ok(http::Response::Ok(body)) = response else {
                continue;
            };
            let Some(next_downloaded) = downloaded.checked_add(body.bytes.len()) else {
                break;
            };
            if next_downloaded > self.limits.max_article_bytes {
                break;
            }
            downloaded = next_downloaded;

            let Ok(permit) = self.decode_limit.clone().acquire_owned().await else {
                break;
            };
            drop(download_permit);
            let source_url = candidate.url.to_string();
            let candidate = candidate.clone();
            let limits = self.limits.clone();
            let task = tokio::task::spawn_blocking(move || {
                // A timed-out blocking task cannot be cancelled. Keep its shared slot until its
                // decoder really exits so subsequent articles remain bounded.
                let _permit = permit;
                prepare_asset(&candidate, body.bytes, &limits)
            });
            let result = tokio::time::timeout(self.limits.decode_timeout, task).await;
            let mut asset = match result {
                Ok(Ok(Ok(asset))) => asset,
                Ok(Ok(Err(error))) => {
                    log::debug!("ignoring article image {source_url}: {error:#}");
                    continue;
                }
                Ok(Err(error)) => {
                    log::debug!("article image decoder task failed: {error}");
                    continue;
                }
                Err(_) => {
                    log::debug!("article image decoder timed out: {source_url}");
                    continue;
                }
            };

            let remaining = self.limits.max_article_bytes.saturating_sub(retained);
            let Some(used) = fit_asset_to_budget(&mut asset, remaining) else {
                continue;
            };
            retained += used;
            assets.push(asset);
        }
        assets
    }
}

/// Fit optional renditions around the required exact master. A non-WebP master only advertises
/// renditions when the full-width WebP survives; otherwise only its tiny progressive placeholder
/// is useful to the renderer.
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
    if asset.master_extension != "webp" && !has_full_width && asset.renditions.len() > 1 {
        asset.renditions.truncate(1);
        used = asset.master_bytes.len() + asset.renditions[0].bytes.len();
    }
    Some(used)
}

/// Validate and decode one response, keeping its exact bytes as the master and deriving only
/// pixel-lossless WebP renditions that fit the configured limits.
pub fn prepare_asset(candidate: &Candidate, bytes: Vec<u8>, limits: &MediaLimits) -> Result<Asset> {
    if bytes.len() > limits.max_file_bytes {
        bail!("image exceeds file limit");
    }
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
        // Without a smaller full-width WebP, only the first useful placeholder is retained.
        // Decide that before paying to resize and encode the other responsive widths.
        let full_width = (format != ImageFormat::WebP)
            .then(|| prepare_rendition(&image, image.width(), limits.max_file_bytes, bytes.len()))
            .flatten();
        let responsive = format == ImageFormat::WebP || full_width.is_some();
        for rendition_width in rendition_widths {
            if let Some(rendition) =
                prepare_rendition(&image, rendition_width, limits.max_file_bytes, bytes.len())
            {
                renditions.push(rendition);
                if !responsive {
                    break;
                }
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
    let hash = crate::model::sha1_hex(bytes);
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
    decoder.max_alloc = Some(limits.max_pixels.saturating_mul(8).min(512 * 1024 * 1024));
    decoder
}

fn validate_dimensions(width: u32, height: u32, limits: &MediaLimits) -> Result<()> {
    if width < MIN_AXIS
        || height < MIN_AXIS
        || width > limits.max_axis
        || height > limits.max_axis
        || u64::from(width) * u64::from(height) > limits.max_pixels
    {
        bail!("article image dimensions exceed limits");
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
        bail!("stored article image dimensions exceed limits");
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
            [48, 320, 640, 800]
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
        assert_eq!(files.len(), 5);
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
        assert_eq!(asset.renditions.len(), 2);
        assert_eq!(
            (asset.renditions[0].width, asset.renditions[0].height),
            (48, 29)
        );
        assert_eq!(
            (asset.renditions[1].width, asset.renditions[1].height),
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
    fn compact_masters_keep_a_tiny_placeholder_without_offering_incomplete_sources() {
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
        assert_eq!(
            asset.renditions.first().map(|rendition| rendition.width),
            Some(48)
        );
        assert_eq!(asset.renditions.len(), 1);
        assert_eq!(
            RENDITION_ENCODE_COUNT.get(),
            2,
            "only encode the full-width candidate and the retained placeholder"
        );
        assert!(
            asset
                .renditions
                .iter()
                .all(|rendition| rendition.width < asset.width)
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
    fn stored_legacy_master_derives_a_lossless_placeholder_without_rewriting_it() {
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
        assert_eq!(restored.renditions.len(), 1);
        assert_eq!(restored.renditions[0].width, PLACEHOLDER_WIDTH);
        let expected = source
            .resize_exact(
                restored.renditions[0].width,
                restored.renditions[0].height,
                image::imageops::FilterType::Lanczos3,
            )
            .to_rgba8();
        let decoded = image::load_from_memory(&restored.renditions[0].bytes)
            .unwrap()
            .to_rgba8();
        assert_eq!(decoded, expected);
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
            .encode_frames([image::Frame::new(red), image::Frame::new(green)])
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
    fn malformed_tiny_and_dimension_bomb_inputs_are_rejected() {
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
            .is_err()
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

        assert_eq!(used, 8);
        assert_eq!(asset.renditions.len(), 1);
        assert_eq!(asset.renditions[0].width, PLACEHOLDER_WIDTH);
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
                decode_concurrency: 2,
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
