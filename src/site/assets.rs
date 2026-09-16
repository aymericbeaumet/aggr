//! Content-addressed site assets: the media each item needs, gathered on worker threads and
//! published once per path in item order, plus the revisioned install-time lists the service
//! worker precaches from.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use anyhow::{Context as _, Result, bail};
use serde::Serialize;

use super::context::{ItemCtx, PreviewCtx};
use crate::content;
use crate::model::Item;
use crate::store::Store;

/// Output paths published by this build and the content hash each file was named after.
pub(super) type Published = BTreeMap<String, String>;

/// Everything the service worker fetches at install, as site paths under `base`: shells, list
/// indexes, the assets needed to render them, the Pagefind bundle, and newest offline items.
pub(super) fn precache_paths(
    base: &str,
    lists: impl IntoIterator<Item = String>,
    assets: &[String],
    item_urls: impl IntoIterator<Item = String>,
    offline_items: usize,
) -> Vec<String> {
    const SHELLS: [&str; 11] = [
        "",
        "aggr.json",
        "updates.json",
        "browse/",
        "categories/",
        "sources/",
        "tags/",
        "preferences/",
        "404.html",
        "offline.html",
        "manifest.webmanifest",
    ];
    let mut seen = BTreeSet::new();
    SHELLS
        .iter()
        .map(|path| path.to_string())
        .chain(lists)
        .chain(
            assets
                .iter()
                .filter(|name| {
                    name.ends_with(".css")
                        || name.ends_with(".js")
                        || name.starts_with("favicon-")
                        || name.starts_with("icon-")
                        || name.starts_with("apple-touch-icon-")
                })
                .map(|name| format!("assets/{name}")),
        )
        .chain(item_urls.into_iter().take(offline_items))
        .map(|path| format!("{base}{path}"))
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PrecacheEntry {
    pub(super) url: String,
    pub(super) revision: String,
    pub(super) required: bool,
}

/// Attach an exact content revision to every install-time resource. A new worker can copy
/// byte-identical responses from the previous precache instead of downloading the whole offline
/// archive after every feed commit.
pub(super) fn precache_entries(
    root: &Path,
    urls: Vec<String>,
    published: &Published,
) -> Result<Vec<PrecacheEntry>> {
    urls.into_iter()
        .map(|url| precache_entry(root, url, published))
        .collect()
}

/// Assets published by this build are content-addressed, so their revision is the hash they were
/// named after and only their presence is checked; rendered pages are read and hashed.
fn precache_entry(root: &Path, url: String, published: &Published) -> Result<PrecacheEntry> {
    let relative = url.trim_start_matches('/');
    if relative.split('/').any(|part| part == "..") {
        bail!("invalid precache path {url:?}");
    }
    let file = if relative.is_empty() {
        root.join("index.html")
    } else if relative.ends_with('/') {
        root.join(relative).join("index.html")
    } else {
        root.join(relative)
    };
    let revision = match published.get(relative) {
        Some(revision) => {
            let metadata = std::fs::metadata(&file)
                .with_context(|| format!("reading precache resource {}", file.display()))?;
            if !metadata.is_file() {
                bail!("precache resource {} is not a file", file.display());
            }
            revision.clone()
        }
        None => {
            let bytes = std::fs::read(&file)
                .with_context(|| format!("reading precache resource {}", file.display()))?;
            crate::model::sha1_hex(&bytes)
        }
    };
    let required = relative.is_empty()
        || relative == "offline.html"
        || (relative.starts_with("items/") && relative.ends_with('/'))
        || (relative.starts_with("assets/")
            && (relative.ends_with(".css") || relative.ends_with(".js")));
    Ok(PrecacheEntry {
        url,
        revision,
        required,
    })
}

#[derive(Serialize)]
pub(super) struct OfflineArticle {
    url: String,
    title: String,
    resources: Vec<PrecacheEntry>,
}

pub(super) fn offline_catalog(
    out: &Path,
    items: &[ItemCtx],
    images: &BTreeMap<String, Vec<content::LocalImage>>,
    published: &Published,
) -> Result<Vec<OfflineArticle>> {
    let mut revisions = BTreeMap::<String, PrecacheEntry>::new();
    items
        .iter()
        .take(1000)
        .map(|item| {
            let mut paths = BTreeSet::from([item.url.clone()]);
            if let Some(preview) = &item.preview {
                paths.insert(preview.url.clone());
            }
            for image in images.get(&item.path).into_iter().flatten() {
                paths.insert(image.original.clone());
                paths.extend(image.variants.iter().map(|variant| variant.url.clone()));
            }
            let resources = paths
                .into_iter()
                .map(|path| {
                    if let Some(entry) = revisions.get(&path) {
                        return Ok(entry.clone());
                    }
                    let entry = precache_entry(out, path.clone(), published)?;
                    revisions.insert(path, entry.clone());
                    Ok(entry)
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(OfflineArticle {
                url: item.url.clone(),
                title: item.title.clone(),
                resources,
            })
        })
        .collect()
}

/// Everything the media phase reads and decodes for one item. Gathering runs on worker threads;
/// [`ItemMedia::publish`] runs in item order on the build thread, the only writer of output
/// files, so the dedupe map and the archive never depend on timing.
pub(super) struct ItemMedia {
    preview: Option<PreparedPreview>,
    assets: Vec<crate::media::Asset>,
}

/// A preview image ready to publish: its content-addressed path, the bytes behind it and the
/// reader context that points at it.
struct PreparedPreview {
    path: String,
    revision: String,
    bytes: Vec<u8>,
    ctx: PreviewCtx,
}

impl ItemMedia {
    /// The validated stored preview, the archived article images, and when the item has no
    /// stored preview and `derive_preview` allows it, a feed thumbnail derived from those images.
    pub(super) fn gather(
        store: &Store,
        item: &Item,
        derive_preview: bool,
        memo: &MediaMemo,
    ) -> Result<Self> {
        let mut preview = None;
        if let Some(stored) = &item.front.preview
            && let Some(bytes) = store.read_preview(item)?
        {
            let extension = Path::new(&stored.file)
                .extension()
                .and_then(|value| value.to_str())
                .context("validated preview has a file extension")?;
            let revision = crate::model::sha1_hex(&bytes);
            let path = format!("assets/previews/{revision}.{extension}");
            preview = Some(PreparedPreview {
                ctx: PreviewCtx {
                    url: path.clone(),
                    width: stored.width,
                    height: stored.height,
                    alt: stored.alt.clone(),
                    color: stored.color.clone(),
                    placeholder: memo.placeholder(&revision, &bytes)?,
                },
                path,
                revision,
                bytes,
            });
        }
        let assets = store.read_image_assets(item)?;
        if preview.is_none() && derive_preview {
            preview = derived_preview(&assets, memo)?;
        }
        Ok(Self { preview, assets })
    }

    /// Writes the preview and every article image once per path; returns the preview context
    /// and the site-local images the article page substitutes for publisher URLs.
    pub(super) fn publish(
        self,
        out: &Path,
        written: &mut Published,
    ) -> Result<(Option<PreviewCtx>, Vec<content::LocalImage>)> {
        let preview = self
            .preview
            .map(|preview| {
                publish_asset(
                    out,
                    written,
                    &preview.path,
                    &preview.revision,
                    &preview.bytes,
                )?;
                Ok::<_, anyhow::Error>(preview.ctx)
            })
            .transpose()?;
        let images = publish_article_images(out, self.assets, written)?;
        Ok((preview, images))
    }
}

/// The first archived article image that yields a feed thumbnail, in the order the article
/// retained them, skipping status badges and oversized bytes.
fn derived_preview(
    assets: &[crate::media::Asset],
    memo: &MediaMemo,
) -> Result<Option<PreparedPreview>> {
    for asset in assets
        .iter()
        .filter(|asset| !crate::media::is_status_badge(&asset.source_url))
        .take(12)
    {
        let retained = asset
            .renditions
            .iter()
            .filter(|image| image.width >= 256 && image.height >= 32)
            .map(|image| image.bytes.as_slice())
            .chain(std::iter::once(asset.master_bytes.as_slice()));
        for bytes in retained.filter(|bytes| bytes.len() <= 5 * 1024 * 1024) {
            let Some(thumbnail) = memo.thumbnail(bytes) else {
                continue;
            };
            let revision = crate::model::sha1_hex(&thumbnail.bytes);
            let path = format!("assets/previews/{revision}.{}", thumbnail.extension);
            return Ok(Some(PreparedPreview {
                ctx: PreviewCtx {
                    url: path.clone(),
                    width: thumbnail.width,
                    height: thumbnail.height,
                    alt: asset.alt.clone(),
                    color: Some(thumbnail.color),
                    placeholder: memo.placeholder(&revision, &thumbnail.bytes)?,
                },
                path,
                revision,
                bytes: thumbnail.bytes,
            }));
        }
    }
    Ok(None)
}

/// Derived previews shared by the gather workers. Thumbnails and placeholders are deterministic
/// functions of their input bytes, so a miss (including two workers deriving the same image at
/// once) costs time, never a different output.
#[derive(Default)]
pub(super) struct MediaMemo {
    thumbnails: Mutex<BTreeMap<String, Option<crate::preview::Thumbnail>>>,
    placeholders: Mutex<BTreeMap<String, crate::media::placeholder::Placeholder>>,
    /// Placeholder receipts that outlive the build, so a warm rebuild decodes no image twice.
    receipts: Option<crate::media::StoredAssetCache>,
}

/// Thumbnails keep their encoded bytes, so only the first distinct images seen are memoised;
/// later repeats derive again rather than grow memory with the archive.
const MEMOISED_THUMBNAILS: usize = 64;

impl MediaMemo {
    pub(super) fn new(receipts: Option<crate::media::StoredAssetCache>) -> Self {
        Self {
            receipts,
            ..Self::default()
        }
    }

    /// Feed thumbnail for one archived image, or `None` when the bytes yield none.
    fn thumbnail(&self, bytes: &[u8]) -> Option<crate::preview::Thumbnail> {
        let key = crate::model::sha1_hex(bytes);
        if let Some(cached) = lock(&self.thumbnails).get(&key) {
            return cached.clone();
        }
        let thumbnail = crate::preview::thumbnail(bytes, None).ok();
        let mut memo = lock(&self.thumbnails);
        if memo.len() < MEMOISED_THUMBNAILS {
            memo.insert(key, thumbnail.clone());
        }
        thumbnail
    }

    fn placeholder(
        &self,
        revision: &str,
        bytes: &[u8],
    ) -> Result<crate::media::placeholder::Placeholder> {
        if let Some(cached) = lock(&self.placeholders).get(revision) {
            return Ok(cached.clone());
        }
        let placeholder = match self
            .receipts
            .as_ref()
            .and_then(|receipts| receipts.cached_placeholder(revision))
        {
            Some(cached) => cached,
            None => {
                let derived = crate::media::placeholder::from_bytes(bytes)?;
                if let Some(receipts) = &self.receipts {
                    receipts.remember_placeholder(revision, &derived);
                }
                derived
            }
        };
        lock(&self.placeholders).insert(revision.to_string(), placeholder.clone());
        Ok(placeholder)
    }
}

fn lock<T>(memo: &Mutex<T>) -> MutexGuard<'_, T> {
    memo.lock().unwrap_or_else(PoisonError::into_inner)
}

fn publish_article_images(
    out: &Path,
    assets: Vec<crate::media::Asset>,
    written: &mut Published,
) -> Result<Vec<content::LocalImage>> {
    let mut published = Vec::with_capacity(assets.len());
    for asset in assets {
        let mut publish = |extension: &str, hash: &str, bytes: &[u8]| -> Result<String> {
            let path = format!("assets/images/{hash}.{extension}");
            publish_asset(out, written, &path, hash, bytes)?;
            Ok(path)
        };
        let original = publish(
            asset.master_extension,
            &asset.master_hash,
            &asset.master_bytes,
        )?;
        let mut variants = asset
            .renditions
            .iter()
            .map(|rendition| {
                Ok(content::LocalImageVariant {
                    url: publish(rendition.extension, &rendition.hash, &rendition.bytes)?,
                    width: rendition.width,
                    height: rendition.height,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if asset.master_extension == "webp"
            && !variants.is_empty()
            && variants.last().map(|variant| variant.width) != Some(asset.width)
        {
            variants.push(content::LocalImageVariant {
                url: original.clone(),
                width: asset.width,
                height: asset.height,
            });
        }
        published.push(content::LocalImage {
            source: asset.source_url,
            original,
            alt: asset.alt,
            variants,
            width: asset.width,
            height: asset.height,
            color: asset.dominant_color,
            placeholder: asset.placeholder,
        });
    }
    Ok(published)
}

/// Writes `bytes` to `path` the first time the build publishes it and records `revision`, the
/// content hash the path was named after, for the precache lists.
fn publish_asset(
    out: &Path,
    written: &mut Published,
    path: &str,
    revision: &str,
    bytes: &[u8],
) -> Result<()> {
    if !written.contains_key(path) {
        super::write(&out.join(path), bytes)?;
        written.insert(path.to_string(), revision.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_immutable_assets_are_written_once_per_build() {
        let root = tempfile::tempdir().unwrap();
        let mut written = Published::new();
        let path = "assets/images/known.png";
        publish_asset(
            root.path(),
            &mut written,
            path,
            "known",
            b"exact source bytes",
        )
        .unwrap();
        let timestamp = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        std::fs::File::options()
            .write(true)
            .open(root.path().join(path))
            .unwrap()
            .set_modified(timestamp)
            .unwrap();
        publish_asset(
            root.path(),
            &mut written,
            path,
            "known",
            b"exact source bytes",
        )
        .unwrap();
        assert_eq!(
            std::fs::metadata(root.path().join(path))
                .unwrap()
                .modified()
                .unwrap(),
            timestamp
        );
        assert_eq!(
            std::fs::read(root.path().join(path)).unwrap(),
            b"exact source bytes"
        );
        assert_eq!(
            written,
            Published::from([(path.to_string(), "known".to_string())])
        );
    }

    #[test]
    fn precache_lists_shells_lists_assets_and_the_newest_items() {
        let paths = precache_paths(
            "/repo/",
            ["sources/a/".to_string(), "sources/a/".to_string()],
            &["style.css".to_string()],
            ["items/a/1/".to_string(), "items/a/2/".to_string()],
            1,
        );
        assert_eq!(paths[0], "/repo/");
        assert!(paths.contains(&"/repo/aggr.json".to_string()));
        assert!(paths.contains(&"/repo/offline.html".to_string()));
        assert!(paths.contains(&"/repo/browse/".to_string()));
        assert!(paths.contains(&"/repo/preferences/".to_string()));
        assert!(!paths.contains(&"/repo/settings/".to_string()));
        assert!(paths.contains(&"/repo/sources/a/".to_string()));
        assert!(paths.contains(&"/repo/assets/style.css".to_string()));
        assert!(paths.contains(&"/repo/items/a/1/".to_string()));
        assert!(!paths.contains(&"/repo/items/a/2/".to_string()));
        assert_eq!(
            paths
                .iter()
                .filter(|path| *path == "/repo/sources/a/")
                .count(),
            1
        );
    }

    #[test]
    fn precache_entries_revision_every_resource_and_require_the_app_shell() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("assets")).unwrap();
        std::fs::create_dir_all(dir.path().join("items/a/1")).unwrap();
        std::fs::write(dir.path().join("index.html"), "home").unwrap();
        std::fs::write(dir.path().join("offline.html"), "offline").unwrap();
        std::fs::write(dir.path().join("assets/style-a.css"), "css").unwrap();
        std::fs::write(dir.path().join("items/a/1/index.html"), "item").unwrap();

        let entries = precache_entries(
            dir.path(),
            vec![
                "".into(),
                "offline.html".into(),
                "assets/style-a.css".into(),
                "items/a/1/".into(),
            ],
            &Published::new(),
        )
        .unwrap();
        assert!(entries[0].required);
        assert!(entries[1].required);
        assert!(entries[2].required);
        assert!(entries[3].required);
        assert_eq!(entries[0].revision, crate::model::sha1_hex(b"home"));
        assert_eq!(entries[3].revision, crate::model::sha1_hex(b"item"));
    }

    /// A published asset is content-addressed: its recorded hash is the revision, without
    /// reading the file again, but the file must still be there and unpublished paths still hash.
    #[test]
    fn published_assets_reuse_their_recorded_revision_and_must_exist() {
        let dir = tempfile::tempdir().unwrap();
        let mut published = Published::new();
        let image = "assets/images/0123456789abcdef0123456789abcdef01234567.webp";
        publish_asset(
            dir.path(),
            &mut published,
            image,
            "0123456789abcdef0123456789abcdef01234567",
            b"encoded pixels",
        )
        .unwrap();
        std::fs::write(dir.path().join("offline.html"), "offline").unwrap();

        let entries = precache_entries(
            dir.path(),
            vec![image.into(), "/offline.html".into()],
            &published,
        )
        .unwrap();
        assert_eq!(
            entries[0],
            PrecacheEntry {
                url: image.into(),
                revision: "0123456789abcdef0123456789abcdef01234567".into(),
                required: false,
            }
        );
        assert_eq!(entries[1].revision, crate::model::sha1_hex(b"offline"));

        published.insert("assets/images/missing.png".into(), "f".repeat(40));
        let error = precache_entries(
            dir.path(),
            vec!["assets/images/missing.png".into()],
            &published,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("missing.png"), "{error:#}");
    }

    #[test]
    fn placeholders_are_remembered_across_builds_sharing_a_receipt_cache() {
        let pixels = image::RgbaImage::from_fn(64, 40, |x, y| {
            image::Rgba([(x * 4) as u8, (y * 6) as u8, ((x + y) * 2) as u8, 255])
        });
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(pixels)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let bytes = encoded.into_inner();
        let revision = crate::model::sha1_hex(&bytes);
        let cache = tempfile::tempdir().unwrap();
        let receipts = crate::media::StoredAssetCache::new(cache.path());
        assert!(receipts.cached_placeholder(&revision).is_none());

        let first = MediaMemo::new(Some(receipts.clone()));
        let derived = first.placeholder(&revision, &bytes).unwrap();
        assert_eq!(
            receipts.cached_placeholder(&revision),
            Some(derived.clone())
        );

        // A later build with a fresh memo reuses the receipt without decoding: corrupt bytes
        // would otherwise fail, so a match proves the cached path was taken.
        let later = MediaMemo::new(Some(receipts));
        let reused = later.placeholder(&revision, b"not an image").unwrap();
        assert_eq!(reused, derived);
        assert!(
            MediaMemo::default()
                .placeholder(&revision, b"not an image")
                .is_err(),
            "without receipts the bytes must decode"
        );
    }

    #[test]
    fn publishing_reconstructed_article_images_does_not_decode_again() {
        let pixels = image::RgbaImage::from_fn(400, 240, |x, y| {
            image::Rgba([
                ((x * 37 + y * 11) % 256) as u8,
                ((x * 13 + y * 41) % 256) as u8,
                ((x * 29 + y * 23) % 256) as u8,
                255,
            ])
        });
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(pixels)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let asset = crate::media::prepare_asset(
            &crate::media::Candidate {
                url: url::Url::parse("https://publisher.example/diagram.png").unwrap(),
                alt: Some("Diagram".into()),
            },
            encoded.into_inner(),
            &crate::media::MediaLimits::default(),
        )
        .unwrap();
        assert!(!asset.renditions.is_empty());
        let out = tempfile::tempdir().unwrap();

        crate::media::reset_stored_decode_count();
        let mut written = Published::new();
        let published =
            publish_article_images(out.path(), vec![asset.clone()], &mut written).unwrap();

        assert_eq!(crate::media::stored_decode_count(), 0);
        assert_eq!(published.len(), 1);
        assert!(out.path().join(&published[0].original).is_file());
        assert!(
            published[0]
                .variants
                .iter()
                .all(|variant| out.path().join(&variant.url).is_file())
        );
        // Every published path records the hash it was named after as its precache revision.
        assert_eq!(written[&published[0].original], asset.master_hash);
        for (variant, rendition) in published[0].variants.iter().zip(&asset.renditions) {
            assert_eq!(written[&variant.url], rendition.hash);
        }
    }

    #[test]
    fn memoised_previews_are_shared_and_bounded() {
        let memo = MediaMemo::default();
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 75)
            .encode_image(&image::DynamicImage::new_rgb8(320, 160))
            .unwrap();
        let first = memo.thumbnail(&jpeg).unwrap();
        let second = memo.thumbnail(&jpeg).unwrap();
        assert_eq!(first, second);
        assert!(memo.thumbnail(b"not an image").is_none());
        assert_eq!(lock(&memo.thumbnails).len(), 2);
        let revision = crate::model::sha1_hex(&first.bytes);
        let placeholder = memo.placeholder(&revision, &first.bytes).unwrap();
        assert_eq!(
            memo.placeholder(&revision, &first.bytes).unwrap(),
            placeholder
        );
        assert_eq!(lock(&memo.placeholders).len(), 1);
    }
}
