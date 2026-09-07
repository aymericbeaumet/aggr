//! Optional, bounded local article thumbnails. Publisher URLs never reach the browser.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use image::{DynamicImage, GenericImageView as _, ImageDecoder as _, ImageFormat, ImageReader};
use scraper::{Html, Selector};
use tokio::sync::Semaphore;
use tokio::time::Instant;
use url::Url;

use crate::config::{FetchConfig, Source};
use crate::http;
use crate::model::Preview;

pub const MAX_BYTES: usize = 384 * 1024;
const MAX_INPUT_BYTES: usize = 5 * 1024 * 1024;
const MAX_PIXELS: u64 = 16_000_000;
const MAX_AXIS: u32 = 8192;
const MAX_CANDIDATES: usize = 3;
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub url: String,
    pub alt: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Thumbnail {
    pub bytes: Vec<u8>,
    pub extension: &'static str,
    pub width: u32,
    pub height: u32,
    pub alt: Option<String>,
    pub color: String,
}

impl Thumbnail {
    pub fn metadata(&self, stem: &str) -> Preview {
        Preview {
            file: format!(
                "{stem}.preview-{}.{}",
                &crate::model::sha1_hex(&self.bytes)[..12],
                self.extension
            ),
            width: self.width,
            height: self.height,
            alt: self.alt.clone(),
            color: Some(self.color.clone()),
        }
    }
}

pub struct Fetcher {
    client: http::Client,
    limit: Arc<Semaphore>,
}

impl Fetcher {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: http::Client::new(&FetchConfig {
                timeout_secs: 10,
                max_body_bytes: MAX_INPUT_BYTES,
                retries: 0,
                ..FetchConfig::default()
            })?,
            limit: Arc::new(Semaphore::new(2)),
        })
    }

    /// Prefer an exact master already fetched for this article. Candidate ordering is unchanged,
    /// and a master that cannot satisfy the tighter preview limits simply yields to the next one.
    pub async fn fetch_with_assets(
        &self,
        candidates: &[Candidate],
        source: &Source,
        assets: &[crate::media::Asset],
    ) -> Option<Thumbnail> {
        self.fetch_with_assets_timeout(candidates, source, assets, FETCH_TIMEOUT)
            .await
    }

    async fn fetch_with_assets_timeout(
        &self,
        candidates: &[Candidate],
        source: &Source,
        assets: &[crate::media::Asset],
        timeout: Duration,
    ) -> Option<Thumbnail> {
        let candidates = candidates
            .iter()
            .take(MAX_CANDIDATES)
            .filter_map(|candidate| safe_url(&candidate.url, None).map(|url| (candidate, url)))
            .collect::<Vec<_>>();
        let deadline = Instant::now() + timeout;
        for (index, (candidate, url)) in candidates.iter().enumerate() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let attempts = u32::try_from(candidates.len() - index).unwrap_or(1);
            let attempt_timeout = remaining / attempts;
            if attempt_timeout.is_zero() {
                break;
            }
            let reusable = assets
                .iter()
                .find(|asset| asset.source_url == url.as_str())
                .map(|asset| asset.master_bytes.as_slice());
            if let Ok(Some(thumbnail)) = tokio::time::timeout(
                attempt_timeout,
                self.fetch_candidate(candidate, url, source, reusable),
            )
            .await
            {
                return Some(thumbnail);
            }
        }
        None
    }

    async fn fetch_candidate(
        &self,
        candidate: &Candidate,
        url: &Url,
        source: &Source,
        reusable: Option<&[u8]>,
    ) -> Option<Thumbnail> {
        let permit = self.limit.clone().acquire_owned().await.ok()?;
        let bytes = if let Some(bytes) = reusable {
            if bytes.len() > MAX_INPUT_BYTES {
                return None;
            }
            bytes.to_vec()
        } else {
            let response = self
                .client
                .get(http::Request {
                    url,
                    headers: http::source_headers(source, url),
                    etag: None,
                    last_modified: None,
                })
                .await;
            let Ok(http::Response::Ok(body)) = response else {
                return None;
            };
            body.bytes
        };
        let alt = candidate.alt.clone();
        tokio::task::spawn_blocking(move || {
            // A timed-out task is not cancellable. Its permit keeps all subsequent work bounded
            // until the decoder really exits.
            let _permit = permit;
            thumbnail(&bytes, alt)
        })
        .await
        .ok()?
        .ok()
    }
}

fn safe_url(value: &str, base: Option<&Url>) -> Option<Url> {
    let mut url = base
        .map_or_else(|| Url::parse(value.trim()), |base| base.join(value.trim()))
        .ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
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

/// Ordered, deduplicated and safe candidates, capped before any network requests begin.
pub fn candidates(explicit: &[Candidate], html: Option<&str>, base: &Url) -> Vec<Candidate> {
    let mut positions = BTreeMap::<String, usize>::new();
    let mut candidates = Vec::<Candidate>::new();
    for candidate in explicit.iter().cloned().chain(
        html.into_iter()
            .flat_map(|html| html_candidates(html, base)),
    ) {
        let Some(url) = safe_url(&candidate.url, Some(base)).map(|url| url.to_string()) else {
            continue;
        };
        let alt = clean_alt(candidate.alt.as_deref());
        if let Some(index) = positions.get(&url).copied() {
            if candidates[index].alt.is_none() {
                candidates[index].alt = alt;
            }
        } else if candidates.len() < 3 {
            positions.insert(url.clone(), candidates.len());
            candidates.push(Candidate { url, alt });
        }
    }
    candidates
}

#[derive(Debug, Default)]
pub(crate) struct HtmlCandidateGroups {
    metadata: Vec<Candidate>,
    body: Vec<Candidate>,
}

/// Metadata beats article-body images; callers prepend explicit feed images.
pub fn html_candidates(html: &str, base: &Url) -> Vec<Candidate> {
    ordered_article_candidates(&[], html_candidate_groups(html, base), None)
}

pub(crate) fn ordered_article_candidates(
    explicit: &[Candidate],
    groups: HtmlCandidateGroups,
    extracted: Option<String>,
) -> Vec<Candidate> {
    explicit
        .iter()
        .cloned()
        .chain(groups.metadata)
        .chain(extracted.map(|url| Candidate { url, alt: None }))
        .chain(groups.body)
        .collect()
}

pub(crate) fn html_candidate_groups(html: &str, base: &Url) -> HtmlCandidateGroups {
    let normalized = crate::content::normalize_image_sources(html);
    let document = Html::parse_document(&normalized);
    let Ok(meta_selector) = Selector::parse("meta[property], meta[name]") else {
        return HtmlCandidateGroups::default();
    };
    let mut og = Vec::<Candidate>::new();
    let mut twitter = Vec::<Candidate>::new();
    for node in document.select(&meta_selector) {
        let Some(name) = node
            .value()
            .attr("property")
            .or_else(|| node.value().attr("name"))
        else {
            continue;
        };
        let Some(value) = node.value().attr("content") else {
            continue;
        };
        match name.to_ascii_lowercase().as_str() {
            "og:image" | "og:image:url" | "og:image:secure_url" => {
                if let Some(url) = safe_url(value, Some(base)) {
                    og.push(Candidate {
                        url: url.to_string(),
                        alt: None,
                    });
                }
            }
            "og:image:alt" => {
                if let Some(image) = og.last_mut() {
                    image.alt = clean_alt(Some(value));
                }
            }
            "twitter:image" | "twitter:image:src" => {
                if let Some(url) = safe_url(value, Some(base)) {
                    twitter.push(Candidate {
                        url: url.to_string(),
                        alt: None,
                    });
                }
            }
            "twitter:image:alt" => {
                if let Some(image) = twitter.last_mut() {
                    image.alt = clean_alt(Some(value));
                }
            }
            _ => {}
        }
    }
    let metadata = og.into_iter().chain(twitter).take(24).collect::<Vec<_>>();
    let image_selector = ["article img", "main img", "body img"]
        .into_iter()
        .filter_map(|selector| Selector::parse(selector).ok())
        .find(|selector| document.select(selector).next().is_some());
    let Some(image_selector) = image_selector else {
        return HtmlCandidateGroups {
            metadata,
            body: Vec::new(),
        };
    };
    let body_limit = 24_usize.saturating_sub(metadata.len());
    let body = document
        .select(&image_selector)
        .filter_map(|node| {
            if ["width", "height"].into_iter().any(|name| {
                node.value()
                    .attr(name)
                    .and_then(|value| value.parse::<u32>().ok())
                    .is_some_and(|size| size < 32)
            }) {
                return None;
            }
            let value = node
                .value()
                .attr("src")
                .filter(|value| !value.trim().is_empty() && !value.starts_with("data:"))
                .or_else(|| node.value().attr("data-src"))
                .or_else(|| node.value().attr("data-lazy-src"))?;
            let url = safe_url(value, Some(base))?;
            Some(Candidate {
                url: url.to_string(),
                alt: clean_alt(node.value().attr("alt")),
            })
        })
        .take(body_limit)
        .collect();
    HtmlCandidateGroups { metadata, body }
}

pub fn thumbnail(bytes: &[u8], alt: Option<String>) -> Result<Thumbnail> {
    let (image, orientation) = decode(bytes)?;
    let mut small = if image.width() > 256 || image.height() > 256 {
        image.resize(256, 256, image::imageops::FilterType::Lanczos3)
    } else {
        image
    };
    // Rotate only the bounded thumbnail, avoiding a second full-resolution allocation.
    small.apply_orientation(orientation);
    let color = dominant_color(&small);
    let transparent = small.has_alpha() && small.pixels().any(|(_, _, pixel)| pixel[3] < 255);
    let mut output = Vec::new();
    let encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut output);
    if transparent {
        encoder
            .encode(
                small.as_bytes(),
                small.width(),
                small.height(),
                small.color().into(),
            )
            .context("encoding transparent preview")?;
    } else {
        let rgb = small.to_rgb8();
        encoder
            .encode(
                &rgb,
                small.width(),
                small.height(),
                image::ExtendedColorType::Rgb8,
            )
            .context("encoding preview")?;
    }
    if output.len() > MAX_BYTES {
        bail!("encoded preview exceeds size limit");
    }
    Ok(Thumbnail {
        bytes: output,
        extension: "webp",
        width: small.width(),
        height: small.height(),
        alt: clean_alt(alt.as_deref()),
        color,
    })
}

fn dominant_color(image: &DynamicImage) -> String {
    let sample = image
        .resize_exact(1, 1, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let pixel = sample.get_pixel(0, 0);
    format!("#{:02x}{:02x}{:02x}", pixel[0], pixel[1], pixel[2])
}

/// Check a small companion from an untrusted mirror before copying or serving its bytes.
pub fn validate_stored(bytes: &[u8], metadata: &Preview) -> Result<&'static str> {
    if bytes.len() > MAX_BYTES {
        bail!("preview exceeds size limit");
    }
    let format = image::guess_format(bytes).context("recognizing preview format")?;
    let extension = match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::WebP => "webp",
        _ => bail!("preview must be JPEG or WebP"),
    };
    let digest = crate::model::sha1_hex(bytes);
    if !metadata
        .file
        .ends_with(&format!(".preview-{}.{extension}", &digest[..12]))
    {
        bail!("preview filename does not match its bytes");
    }
    let (width, height) = ImageReader::with_format(Cursor::new(bytes), format).into_dimensions()?;
    if width != metadata.width || height != metadata.height || width > 320 || height > 320 {
        bail!("preview dimensions do not match its metadata");
    }
    decode(bytes)?;
    Ok(extension)
}

fn decode(bytes: &[u8]) -> Result<(DynamicImage, image::metadata::Orientation)> {
    if bytes.len() > MAX_INPUT_BYTES {
        bail!("preview input exceeds size limit");
    }
    let format = image::guess_format(bytes).context("recognizing preview image")?;
    if !matches!(
        format,
        ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::Gif | ImageFormat::WebP
    ) {
        bail!("unsupported preview image format");
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_AXIS);
    limits.max_image_height = Some(MAX_AXIS);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .context("reading preview dimensions")?;
    let (width, height) = decoder.dimensions();
    if width < 2 || height < 2 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        bail!("preview dimensions exceed limits");
    }
    let orientation = decoder
        .orientation()
        .context("reading preview orientation")?;
    let image = DynamicImage::from_decoder(decoder).context("decoding preview image")?;
    Ok((image, orientation))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    fn png(width: u32, height: u32, alpha: u8) -> Vec<u8> {
        let image = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(
            width,
            height,
            Rgba([40, 90, 140, alpha]),
        ));
        let mut output = Cursor::new(Vec::new());
        image.write_to(&mut output, ImageFormat::Png).unwrap();
        output.into_inner()
    }

    #[test]
    fn prioritizes_safe_feed_then_metadata_then_body_candidates() {
        let base = Url::parse("https://example.com/articles/post").unwrap();
        let explicit = vec![Candidate {
            url: "/feed.png".into(),
            alt: Some("  Feed  image ".into()),
        }];
        let html = r#"<meta property="og:image" content="/og.png"><meta property="og:image:alt" content="Article cover"><meta name="twitter:image" content="/twitter.png"><img src="/body.png">"#;
        let found = candidates(&explicit, Some(html), &base);
        assert_eq!(
            found.iter().map(|c| c.url.as_str()).collect::<Vec<_>>(),
            [
                "https://example.com/feed.png",
                "https://example.com/og.png",
                "https://example.com/twitter.png"
            ]
        );
        assert_eq!(found[0].alt.as_deref(), Some("Feed image"));
        assert_eq!(found[1].alt.as_deref(), Some("Article cover"));
    }

    #[test]
    fn extracted_image_precedes_generic_page_body_fallbacks() {
        let base = Url::parse("https://example.com/articles/post").unwrap();
        let explicit = vec![Candidate {
            url: "https://feed.example/cover.png".into(),
            alt: Some("Feed cover".into()),
        }];
        let html = r#"<meta property="og:image" content="/og.png"><meta name="twitter:image" content="/twitter.png"><article><img src="/body.png"></article>"#;
        let found = ordered_article_candidates(
            &explicit,
            html_candidate_groups(html, &base),
            Some("https://example.com/extracted.png".into()),
        );
        assert_eq!(
            found
                .iter()
                .map(|candidate| candidate.url.as_str())
                .collect::<Vec<_>>(),
            [
                "https://feed.example/cover.png",
                "https://example.com/og.png",
                "https://example.com/twitter.png",
                "https://example.com/extracted.png",
                "https://example.com/body.png",
            ]
        );
    }

    #[test]
    fn skips_unsafe_duplicates_and_tracking_pixels() {
        let base = Url::parse("https://example.com/a/").unwrap();
        let html = r#"<meta property="og:image" content="javascript:alert(1)"><img src="data:image/png,abc"><img src="https://user:pass@example.com/private.png"><img src="/pixel.gif" width="1"><img src="photo.jpg"><img src="photo.jpg#duplicate">"#;
        assert_eq!(
            candidates(&[], Some(html), &base),
            vec![Candidate {
                url: "https://example.com/a/photo.jpg".into(),
                alt: None
            }]
        );
    }

    #[test]
    fn discovers_responsive_lazy_and_picture_body_candidates() {
        let base = Url::parse("https://example.com/articles/post").unwrap();
        let html = r#"
          <article>
            <img alt="Responsive" src="/placeholder.gif" srcset="/small.jpg 320w, /large.jpg 1280w">
            <img alt="Lazy set" src="data:image/gif;base64,AAAA" data-srcset="/lazy-small.webp 1x, /lazy-large.webp 2x">
            <picture>
              <source type="image/avif" srcset="/hero.avif 2x">
              <source type="image/webp" data-srcset="/hero-small.webp 320w, /hero-large.webp 1200w">
              <img alt="Picture" src="data:image/gif;base64,AAAA">
            </picture>
            <img alt="Lazy source" src="data:image/gif;base64,AAAA" data-lazy-src="/lazy.jpg">
          </article>
        "#;
        let found = html_candidates(html, &base);
        assert_eq!(
            found,
            [
                ("https://example.com/large.jpg", "Responsive"),
                ("https://example.com/lazy-large.webp", "Lazy set"),
                ("https://example.com/hero-large.webp", "Picture"),
                ("https://example.com/lazy.jpg", "Lazy source"),
            ]
            .into_iter()
            .map(|(url, alt)| Candidate {
                url: url.into(),
                alt: Some(alt.into()),
            })
            .collect::<Vec<_>>()
        );
    }

    #[test]
    fn duplicate_candidates_merge_later_alt_metadata_after_the_network_cap() {
        let base = Url::parse("https://example.com/articles/post").unwrap();
        let explicit = ["/same.jpg", "/second.jpg", "/third.jpg"].map(|url| Candidate {
            url: url.into(),
            alt: None,
        });
        let html = r#"<meta property="og:image" content="/same.jpg"><meta property="og:image:alt" content="  Descriptive   cover  ">"#;
        let found = candidates(&explicit, Some(html), &base);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].url, "https://example.com/same.jpg");
        assert_eq!(found[0].alt.as_deref(), Some("Descriptive cover"));
    }

    #[test]
    fn opaque_thumbnail_uses_lossless_webp_with_correct_aspect_ratio() {
        let preview = thumbnail(&png(640, 400, 255), Some("Cover".into())).unwrap();
        assert_eq!(
            (preview.width, preview.height, preview.extension),
            (256, 160, "webp")
        );
        assert!(preview.bytes.len() <= MAX_BYTES);
        assert_eq!(
            validate_stored(&preview.bytes, &preview.metadata("item")).unwrap(),
            "webp"
        );
        assert_eq!(preview.color, "#285a8c");
    }

    #[test]
    fn preserves_transparency_as_webp_and_does_not_upscale() {
        let preview = thumbnail(&png(120, 80, 90), None).unwrap();
        assert_eq!(
            (preview.width, preview.height, preview.extension),
            (120, 80, "webp")
        );
        let decoded = image::load_from_memory(&preview.bytes).unwrap().to_rgba8();
        assert_eq!(decoded.get_pixel(0, 0)[3], 90);
    }

    #[test]
    fn stored_preview_must_match_its_content_addressed_filename() {
        let preview = thumbnail(&png(64, 32, 255), None).unwrap();
        let mut metadata = preview.metadata("article");
        assert!(validate_stored(&preview.bytes, &metadata).is_ok());
        metadata.file = "article.preview-000000000000.jpg".into();
        assert!(metadata.is_valid_for("article"));
        assert!(validate_stored(&preview.bytes, &metadata).is_err());
    }

    #[test]
    fn noisy_images_stay_lossless_at_the_display_resolution() {
        let mut state = 1_u32;
        let pixels = ImageBuffer::from_fn(320, 320, |_, _| {
            image::Rgb(std::array::from_fn(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            }))
        });
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(pixels)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        let preview = thumbnail(&bytes.into_inner(), None).unwrap();
        assert_eq!((preview.width, preview.height), (256, 256));
        assert!(preview.bytes.len() <= MAX_BYTES);
    }

    #[test]
    fn applies_orientation_and_strips_original_metadata() {
        use image::ImageEncoder as _;
        let original = DynamicImage::new_rgb8(80, 40);
        // Little-endian TIFF: one orientation entry, rotate 90 degrees clockwise.
        let exif = vec![
            b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
        ];
        let mut bytes = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new(&mut bytes);
        encoder.set_exif_metadata(exif).unwrap();
        encoder.encode_image(&original).unwrap();
        let preview = thumbnail(&bytes, None).unwrap();
        assert_eq!((preview.width, preview.height), (40, 80));
        assert!(!preview.bytes.windows(6).any(|bytes| bytes == b"Exif\0\0"));
    }

    #[test]
    fn turns_an_animation_into_its_first_frame() {
        let red = ImageBuffer::from_pixel(40, 20, Rgba([240, 10, 10, 255]));
        let green = ImageBuffer::from_pixel(40, 20, Rgba([10, 240, 10, 255]));
        let mut bytes = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut bytes)
            .encode_frames([image::Frame::new(red), image::Frame::new(green)])
            .unwrap();
        let preview = thumbnail(&bytes, None).unwrap();
        assert_eq!(preview.extension, "webp");
        let decoded = image::load_from_memory(&preview.bytes).unwrap().to_rgb8();
        let pixel = decoded.get_pixel(10, 10);
        assert!(pixel[0] > 200 && pixel[1] < 30);
    }

    #[test]
    fn rejects_non_images_pixels_and_excess_dimensions() {
        assert!(thumbnail(b"<svg xmlns='http://www.w3.org/2000/svg'></svg>", None).is_err());
        assert!(thumbnail(&png(1, 1, 255), None).is_err());
        assert!(thumbnail(&png(MAX_AXIS + 1, 2, 255), None).is_err());
        assert!(thumbnail(&vec![0; MAX_INPUT_BYTES + 1], None).is_err());
    }

    #[test]
    fn article_images_beat_navigation_logos() {
        let base = Url::parse("https://example.com/post").unwrap();
        let html = r#"<body><header><img src="/logo.png"></header><article><img src="/photo.png" alt="The article"></article></body>"#;
        let found = candidates(&[], Some(html), &base);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].url, "https://example.com/photo.png");
    }

    #[tokio::test]
    async fn skips_failed_images_and_never_tries_more_than_three() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let first = server.mock(|when, then| {
            when.path("/first");
            then.status(200).body("<svg></svg>");
        });
        let second = server.mock(|when, then| {
            when.path("/second");
            then.status(404);
        });
        let third = server.mock(|when, then| {
            when.path("/third");
            then.status(200).body(png(64, 32, 255));
        });
        let fourth = server.mock(|when, then| {
            when.path("/fourth");
            then.status(200).body(png(64, 32, 255));
        });
        let config = crate::config::Config::parse(&format!(
            "[[sources]]\nurl = {:?}\n",
            server.url("/feed")
        ))
        .unwrap();
        let source = config.sources().unwrap().remove(0);
        let candidates = ["first", "second", "third", "fourth"].map(|path| Candidate {
            url: server.url(format!("/{path}")),
            alt: None,
        });
        let preview = Fetcher::new()
            .unwrap()
            .fetch_with_assets(&candidates, &source, &[])
            .await
            .unwrap();
        assert_eq!(preview.width, 64);
        first.assert_calls(1);
        second.assert_calls(1);
        third.assert_calls(1);
        fourth.assert_calls(0);
    }

    #[tokio::test]
    async fn reuses_a_matching_article_master_without_downloading_it_again() {
        use httpmock::prelude::*;

        let server = MockServer::start_async().await;
        let image_url = server.url("/cover.png");
        let network = server
            .mock_async(|when, then| {
                when.path("/cover.png");
                then.status(500);
            })
            .await;
        let bytes = png(640, 400, 255);
        let asset = crate::media::prepare_asset(
            &crate::media::Candidate {
                url: Url::parse(&image_url).unwrap(),
                alt: None,
            },
            bytes,
            &crate::media::MediaLimits::default(),
        )
        .unwrap();
        let config = crate::config::Config::parse(&format!(
            "[[sources]]\nurl = {:?}\n",
            server.url("/feed")
        ))
        .unwrap();
        let source = config.sources().unwrap().remove(0);
        let preview = Fetcher::new()
            .unwrap()
            .fetch_with_assets(
                &[Candidate {
                    url: image_url,
                    alt: Some("Cover".into()),
                }],
                &source,
                &[asset],
            )
            .await
            .unwrap();

        assert_eq!((preview.width, preview.height), (256, 160));
        assert_eq!(preview.alt.as_deref(), Some("Cover"));
        network.assert_calls_async(0).await;
    }

    #[tokio::test]
    async fn a_slow_candidate_cannot_consume_the_next_candidates_time() {
        use httpmock::prelude::*;

        let server = MockServer::start_async().await;
        let slow = server
            .mock_async(|when, then| {
                when.path("/slow.png");
                then.status(200)
                    .delay(Duration::from_millis(300))
                    .body(png(64, 32, 255));
            })
            .await;
        let fast = server
            .mock_async(|when, then| {
                when.path("/fast.png");
                then.status(200).body(png(96, 48, 255));
            })
            .await;
        let config = crate::config::Config::parse(&format!(
            "[[sources]]\nurl = {:?}\n",
            server.url("/feed")
        ))
        .unwrap();
        let source = config.sources().unwrap().remove(0);
        let candidates = ["slow.png", "fast.png"].map(|path| Candidate {
            url: server.url(format!("/{path}")),
            alt: None,
        });

        let preview = Fetcher::new()
            .unwrap()
            .fetch_with_assets_timeout(&candidates, &source, &[], Duration::from_millis(400))
            .await
            .unwrap();

        assert_eq!((preview.width, preview.height), (96, 48));
        slow.assert_calls_async(1).await;
        fast.assert_calls_async(1).await;
    }

    #[tokio::test]
    async fn only_sends_source_headers_to_images_on_the_configured_origin() {
        use httpmock::prelude::*;
        let source_server = MockServer::start();
        let image_server = MockServer::start();
        let same_origin = source_server.mock(|when, then| {
            when.path("/image")
                .header("authorization", "Bearer private")
                .header("x-api-key", "private");
            then.status(200).body(png(64, 32, 255));
        });
        let other_origin = image_server.mock(|when, then| {
            when.path("/image")
                .header_missing("authorization")
                .header_missing("x-api-key");
            then.status(200).body(png(64, 32, 255));
        });
        let config = crate::config::Config::parse(&format!(
            "[[sources]]\nurl = {:?}\nheaders = {{ Authorization = \"Bearer private\", X-Api-Key = \"private\" }}\n",
            source_server.url("/feed"),
        )).unwrap();
        let source = config.sources().unwrap().remove(0);
        let fetcher = Fetcher::new().unwrap();
        for server in [&source_server, &image_server] {
            assert!(
                fetcher
                    .fetch_with_assets(
                        &[Candidate {
                            url: server.url("/image"),
                            alt: None,
                        }],
                        &source,
                        &[],
                    )
                    .await
                    .is_some()
            );
        }
        same_origin.assert_calls(1);
        other_origin.assert_calls(1);
    }
}
