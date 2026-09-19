//! Repairs that run after a source's feed pass, on items the feed may no longer list: missing
//! image and preview companions, feed-only captures whose original page is retried, recording
//! durations backfilled from their pages, and podcast episodes whose enclosure moved.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use futures_util::{StreamExt as _, stream};

use super::plan::{Planned, merge_image_metadata, persist_item, prepare_item, use_existing_path};
use super::transaction::SourceTransaction;
use super::{FetchOneContext, Options, heavy_content, podcast, recording};
use crate::config::{ContentMode, Engine, Source};
use crate::content;
use crate::media;
use crate::model::{ContentKind, FrontMatter, Item, RawItem, dedupe_keys};
use crate::preview;
use crate::sources;
use crate::store::{NewItem, Store};

/// Re-derive stored bodies from the HTML retained beside them. Content cleanup added after an item
/// was captured cannot reach it any other way: its source eventually stops listing it, and a build
/// never overrides a stored body. Explicit, like `--refresh`, because it discards hand edits, and
/// bounded to items whose retained HTML is complete so a body can never come back shorter.
pub(super) fn reprocess_stored_bodies(
    source: &Source,
    store: &Store,
    options: &Options,
    mut transaction: Option<&mut SourceTransaction>,
) -> Result<usize> {
    if !options.reprocess || options.dry_run {
        return Ok(0);
    }
    let Some(index) = options
        .existing_paths
        .get()
        .and_then(|index| index.items.get(&source.slug))
    else {
        return Ok(0);
    };
    let mut rewritten = 0;
    for path in index.values().collect::<BTreeSet<_>>() {
        let existing = store.read_item(path)?;
        if existing.front.source != source.slug || existing.front.html_truncated {
            continue;
        }
        let Some(html) = store.read_html(&existing)? else {
            continue;
        };
        let base = url::Url::parse(&existing.front.link).ok();
        let body = content::strip_article_metadata(
            &content::to_markdown(&html, base.as_ref()),
            &existing.front.title,
            existing.front.published,
            &source.slug,
        );
        if body == existing.body || body.trim().is_empty() {
            continue;
        }
        let planned = Planned::from_existing(Item { body, ..existing }, path)?;
        if let Some(transaction) = transaction.as_deref_mut() {
            transaction.track_item(&planned, &RawItem::default())?;
        }
        store.write_item(NewItem {
            dir: &planned.dir,
            stem: &planned.stem,
            front: &planned.front,
            body: &planned.body,
            html: None,
            preview: None,
            images: &[],
        })?;
        log::debug!("{}: re-derived the stored body for {path}", source.slug);
        rewritten += 1;
    }
    if rewritten > 0 {
        log::info!(
            "{}: re-derived {rewritten} stored article body(ies)",
            source.slug
        );
    }
    Ok(rewritten)
}

pub(super) async fn repair_archived_images(
    source: &Source,
    store: &Arc<Store>,
    options: &Options,
    mut transaction: Option<&mut SourceTransaction>,
) -> Result<usize> {
    if !source.images {
        return Ok(0);
    }
    let Some(archives) = options
        .existing_paths
        .get()
        .and_then(|index| index.images.get(&source.slug))
    else {
        return Ok(0);
    };
    let mut repaired = 0;
    for archive in archives {
        let preview_missing = archive
            .preview
            .as_ref()
            .map(|preview| {
                store
                    .preview_file_present(&archive.path, preview)
                    .map(|present| !present)
            })
            .transpose()?
            .unwrap_or(false);
        if !preview_missing
            && archive.candidates.iter().all(|candidate| {
                archive.images.iter().any(|image| {
                    image.source == candidate.url.as_str()
                        && store.image_files_present(&archive.path, image)
                })
            })
        {
            continue;
        }
        let existing = store.read_item(&archive.path)?;
        let preview_missing = existing
            .front
            .preview
            .as_ref()
            .map(|preview| {
                store
                    .preview_file_present(&archive.path, preview)
                    .map(|present| !present)
            })
            .transpose()?
            .unwrap_or(false);
        let mut remaining_slots = media::MediaLimits::default()
            .max_assets
            .saturating_sub(existing.front.images.len());
        let candidates: Vec<_> = archive
            .candidates
            .iter()
            .filter(|candidate| {
                if let Some(image) = existing
                    .front
                    .images
                    .iter()
                    .find(|image| image.source == candidate.url.as_str())
                {
                    return !store.image_files_present(&archive.path, image);
                }
                if remaining_slots == 0 {
                    return false;
                }
                remaining_slots -= 1;
                true
            })
            .cloned()
            .collect();
        let html = store.read_html(&existing)?.unwrap_or_default();
        let base = url::Url::parse(&existing.front.link).ok();
        if (candidates.is_empty() && !preview_missing)
            || candidates.iter().any(|candidate| {
                existing
                    .front
                    .images
                    .iter()
                    .any(|image| image.source == candidate.url.as_str())
                    && options.media_fetcher.recently_failed_archived(
                        candidate,
                        source,
                        &html,
                        base.as_ref(),
                    )
            })
        {
            continue;
        }
        let retained_bytes = existing
            .front
            .images
            .iter()
            .map(|image| store.image_bytes_present(&archive.path, image))
            .fold(0usize, usize::saturating_add);
        let remaining_bytes = media::MediaLimits::default()
            .max_article_bytes
            .saturating_sub(retained_bytes);
        let images = if let Some(base) = base {
            options
                .media_fetcher
                .fetch_archived_with_budget(&candidates, source, remaining_bytes, &html, &base)
                .await
        } else {
            options
                .media_fetcher
                .fetch_with_budget(&candidates, source, remaining_bytes)
                .await
        };
        if images.is_empty() && !preview_missing {
            continue;
        }
        let preview = if preview_missing {
            let retained_images = if images.is_empty() && source.previews {
                store.read_image_assets(&existing)?
            } else {
                Vec::new()
            };
            let candidates = archive
                .candidates
                .iter()
                .map(|candidate| preview::Candidate {
                    url: candidate.url.to_string(),
                    alt: candidate.alt.clone(),
                })
                .collect::<Vec<_>>();
            options
                .preview_fetcher
                .fetch_with_assets(
                    &candidates,
                    source,
                    if images.is_empty() {
                        &retained_images
                    } else {
                        &images
                    },
                )
                .await
        } else {
            None
        };
        let mut planned = Planned::from_existing(existing, &archive.path)?;
        if preview_missing {
            planned.front.preview = preview
                .as_ref()
                .map(|preview| preview.metadata(&planned.stem));
        }
        if planned.front.images.iter().any(|image| {
            !store.image_files_present(&archive.path, image)
                && !images
                    .iter()
                    .any(|replacement| replacement.source_url == image.source)
        }) {
            log::warn!(
                "{}: deferred image repair for {} because a previously archived master is unavailable",
                source.slug,
                archive.path
            );
            continue;
        }
        merge_image_metadata(&mut planned.front.images, &images, &planned.stem);
        let raw = RawItem {
            images,
            preview,
            ..Default::default()
        };
        if !options.dry_run {
            if let Some(transaction) = transaction.as_deref_mut() {
                transaction.track_item(&planned, &raw)?;
            }
            persist_item(store, options, planned, raw)
                .await
                .with_context(|| format!("repairing article companions for {}", archive.path))?;
        }
        repaired += 1;
    }
    Ok(repaired)
}

pub(super) fn reconcile_podcast(
    raw: &RawItem,
    path: &str,
    source: &Source,
    store: &Store,
    options: &Options,
    transaction: Option<&mut SourceTransaction>,
    prospective: &BTreeSet<String>,
) -> Result<Option<(bool, Vec<String>)>> {
    let mut existing = match store.read_item(path) {
        Ok(item) => item,
        Err(_) => return Ok(None),
    };
    if existing.front.source != source.slug
        || !Path::new(path).starts_with(Path::new("items").join(&source.slug))
    {
        return Ok(None);
    }
    let Some(audio) = podcast::audio_change(&existing.front, raw) else {
        return Ok(None);
    };
    let aliases: Vec<_> = dedupe_keys(raw)
        .into_iter()
        .filter(|key| !prospective.contains(key))
        .collect();
    let seconds = raw
        .extra
        .get("duration_seconds")
        .and_then(serde_yaml_ng::Value::as_u64)
        .filter(|seconds| *seconds > 0)
        .filter(|seconds| {
            existing
                .front
                .extra
                .get("duration_seconds")
                .and_then(serde_yaml_ng::Value::as_u64)
                != Some(*seconds)
        });
    let changed = audio.is_some() || seconds.is_some() || !aliases.is_empty();
    if audio.is_some() || seconds.is_some() {
        if let Some(audio) = audio {
            existing
                .front
                .extra
                .insert("audio_url".into(), audio.into());
        }
        if let Some(seconds) = seconds {
            existing
                .front
                .extra
                .insert("duration_seconds".into(), seconds.into());
        }
        let planned = Planned::from_existing(existing, path)?;
        if !options.dry_run {
            if let Some(transaction) = transaction {
                transaction.track_item(&planned, &RawItem::default())?;
            }
            store.write_item(NewItem {
                dir: &planned.dir,
                stem: &planned.stem,
                front: &planned.front,
                body: &planned.body,
                html: None,
                preview: None,
                images: &[],
            })?;
        }
    }
    Ok(Some((changed, aliases)))
}

pub(super) async fn repair_recordings(
    source: &Source,
    context: &FetchOneContext<'_>,
    mut transaction: Option<&mut SourceTransaction>,
) -> Result<usize> {
    if matches!(source.engine, Engine::Aggr { .. }) {
        return Ok(0);
    }
    let Some(items) = context
        .options
        .existing_paths
        .get()
        .and_then(|paths| paths.recordings.get(&source.slug))
    else {
        return Ok(0);
    };
    let mut pending = stream::iter(items.clone())
        .map(|(path, raw)| async move {
            let _permit = context
                .options
                .recording_limit
                .acquire()
                .await
                .context("acquiring recording metadata slot")?;
            // A changed feed may already have filled this value earlier in this transaction.
            let current = context.store.read_item(&path)?;
            let raw = RawItem {
                extra: current.front.extra,
                ..raw
            };
            let seconds = recording::infer(
                &raw,
                source,
                context.client,
                context.cache_dir,
                context.article_failures,
            )
            .await?;
            Ok::<_, anyhow::Error>((path, raw, seconds))
        })
        .buffer_unordered(context.options.article_concurrency);
    let mut repaired = 0;
    while let Some(result) = pending.next().await {
        let (path, mut raw, seconds) = result?;
        if let Some(seconds) = seconds {
            raw.extra.insert("duration_seconds".into(), seconds.into());
            repaired += usize::from(reconcile_duration(
                &raw,
                &path,
                source,
                context.store,
                context.options,
                transaction.as_deref_mut(),
            )?);
        }
    }
    Ok(repaired)
}

const MAX_CAPTURE_RETRIES_PER_RUN: usize = 8;
const MAX_CAPTURE_ATTEMPTS: u32 = 7;
const CAPTURE_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Remembers failed original-page retries so a persistently unavailable page is tried at most
/// once a day and gives up after a week. Success removes the record.
struct CaptureRetries {
    root: PathBuf,
}

impl CaptureRetries {
    fn new(cache_dir: &Path) -> Self {
        Self {
            root: crate::cache::Namespace::CaptureRetries.dir(cache_dir),
        }
    }

    fn path(&self, link: &str) -> PathBuf {
        self.root.join(crate::model::sha1_hex(link.as_bytes()))
    }

    fn due(&self, link: &str) -> bool {
        let path = self.path(link);
        let Ok(metadata) = std::fs::metadata(&path) else {
            return true;
        };
        let attempts = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .unwrap_or(1);
        attempts < MAX_CAPTURE_ATTEMPTS
            && metadata
                .modified()
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_none_or(|age| age >= CAPTURE_RETRY_INTERVAL)
    }

    fn failed(&self, link: &str) {
        let path = self.path(link);
        let attempts = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .unwrap_or(0);
        let _ = std::fs::create_dir_all(&self.root);
        let _ = std::fs::write(path, format!("{}\n", attempts + 1));
    }

    fn succeeded(&self, link: &str) {
        let _ = std::fs::remove_file(self.path(link));
    }
}

/// Retry the original page for archived items that only kept feed content. A challenge, outage,
/// or rate limit at capture time should not permanently leave a summary where the article
/// belongs. The upgrade rewrites body, HTML, and media but keeps the item's identity and dates.
pub(super) async fn repair_feed_captures(
    source: &Source,
    context: &FetchOneContext<'_>,
    mut transaction: Option<&mut SourceTransaction>,
) -> Result<usize> {
    // Sources whose feed content is final by design (light mode, mirrors, Qwen's API-backed blog)
    // are never retried.
    if matches!(source.engine, Engine::Aggr { .. })
        || source.content == ContentMode::Light
        || source.engine.url().is_some_and(sources::qwen::is_blog_url)
        || context.options.dry_run
    {
        return Ok(0);
    }
    let Some(items) = context
        .options
        .existing_paths
        .get()
        .and_then(|paths| paths.captures.get(&source.slug))
    else {
        return Ok(0);
    };
    let retries = CaptureRetries::new(context.cache_dir);
    let due = items
        .iter()
        .filter(|(_, raw)| retries.due(&raw.link))
        .take(MAX_CAPTURE_RETRIES_PER_RUN)
        .cloned()
        .collect::<Vec<_>>();
    log::debug!(
        "{}: retrying {} of {} feed-only captures",
        source.slug,
        due.len(),
        items.len()
    );
    let mut pending = stream::iter(due)
        .map(|(path, raw)| async move {
            let (enriched, kind) = heavy_content(
                &raw,
                source,
                context.client,
                context.cache_dir,
                context.article_failures,
            )
            .await;
            (path, enriched, kind)
        })
        .buffer_unordered(context.options.article_concurrency);
    let mut rewritten = 0;
    while let Some((path, mut raw, kind)) = pending.next().await {
        let existing = context.store.read_item(&path)?;
        if existing.front.source != source.slug {
            continue;
        }
        let placeholder = existing.front.content == ContentKind::Extracted
            && content::is_placeholder_body(&existing.body);
        if kind != ContentKind::Extracted || raw.content_html.is_none() {
            retries.failed(&raw.link);
            if placeholder && raw.summary.is_some() {
                // The page is still a script shell. Its feed summary reads better than the
                // archived "loading…", so the item returns to feed content until a retry succeeds.
                let (raw, mut planned) =
                    prepare_item(raw, source, context.options, ContentKind::Feed).await?;
                use_existing_path(&mut planned, &path)?;
                planned.front.preview = existing.front.preview.clone();
                planned.front.images = existing.front.images.clone();
                planned.front = upgrade_front(existing.front, planned.front);
                if let Some(transaction) = transaction.as_deref_mut() {
                    transaction.track_item(&planned, &raw)?;
                }
                let link = raw.link.clone();
                persist_item(context.store, context.options, planned, raw).await?;
                log::info!(
                    "{}: {} had archived a loading placeholder; keeping its feed summary until the page is available",
                    source.slug,
                    link
                );
                rewritten += 1;
                continue;
            }
            log::debug!(
                "{}: original page still unavailable for {}; keeping feed content",
                source.slug,
                raw.link
            );
            continue;
        }
        if existing.front.content == ContentKind::Extracted && !placeholder {
            continue;
        }
        if source.images
            && let Some(html) = raw.content_html.as_deref()
            && let Ok(base) = url::Url::parse(&raw.link)
        {
            let limits = media::MediaLimits::default();
            let candidates = media::article_candidates(html, &raw.preview_candidates, &base)
                .into_iter()
                .take(limits.max_assets)
                .collect::<Vec<_>>();
            raw.images = context
                .options
                .media_fetcher
                .fetch_with_budget(&candidates, source, limits.max_article_bytes)
                .await;
        }
        let has_stored_preview = context.store.read_preview(&existing)?.is_some();
        if source.previews
            && !has_stored_preview
            && let Ok(base) = url::Url::parse(&raw.link)
        {
            let candidates =
                preview::candidates(&raw.preview_candidates, raw.content_html.as_deref(), &base);
            raw.preview = context
                .options
                .preview_fetcher
                .fetch_with_assets(&candidates, source, &raw.images)
                .await;
        }
        let (raw, mut planned) = prepare_item(raw, source, context.options, kind).await?;
        use_existing_path(&mut planned, &path)?;
        if planned.html.is_some() {
            planned.front.html = Some(format!("{}.html", planned.stem));
        }
        planned.front.preview = match &raw.preview {
            Some(preview) => Some(preview.metadata(&planned.stem)),
            None if has_stored_preview => existing.front.preview.clone(),
            None => None,
        };
        planned.front.images = existing.front.images.clone();
        merge_image_metadata(&mut planned.front.images, &raw.images, &planned.stem);
        planned.front = upgrade_front(existing.front, planned.front);
        if let Some(transaction) = transaction.as_deref_mut() {
            transaction.track_item(&planned, &raw)?;
        }
        let link = raw.link.clone();
        persist_item(context.store, context.options, planned, raw).await?;
        retries.succeeded(&link);
        log::info!(
            "{}: captured the original article for {} after an earlier failure",
            source.slug,
            link
        );
        rewritten += 1;
    }
    Ok(rewritten)
}

pub(super) fn reconcile_duration(
    raw: &RawItem,
    path: &str,
    source: &Source,
    store: &Store,
    options: &Options,
    transaction: Option<&mut SourceTransaction>,
) -> Result<bool> {
    let Some(seconds) = raw
        .extra
        .get("duration_seconds")
        .and_then(serde_yaml_ng::Value::as_u64)
        .filter(|value| *value > 0)
    else {
        return Ok(false);
    };
    let mut item = store.read_item(path)?;
    if item.front.source != source.slug
        || item
            .front
            .extra
            .get("duration_seconds")
            .and_then(serde_yaml_ng::Value::as_u64)
            == Some(seconds)
    {
        return Ok(false);
    }
    item.front
        .extra
        .insert("duration_seconds".into(), seconds.into());
    let planned = Planned::from_existing(item, path)?;
    if !options.dry_run {
        if let Some(transaction) = transaction {
            transaction.track_item(&planned, &RawItem::default())?;
        }
        store.write_item(NewItem {
            dir: &planned.dir,
            stem: &planned.stem,
            front: &planned.front,
            body: &planned.body,
            html: None,
            preview: None,
            images: &[],
        })?;
    }
    Ok(true)
}

/// Extra keys `heavy_content` derives from the original page. Every other key is the item's own.
const PAGE_DERIVED_EXTRA_KEYS: [&str; 2] =
    ["duration_seconds", crate::site::interactive::METADATA_KEY];

/// The front matter of a feed-only capture after its original page finally loaded: `existing` as
/// archived, with only what the page supplies replaced. The upgrade owns the content kind, the
/// HTML sibling and its truncation flag, the preview and image companions, the recording duration
/// and interactive marker the page revealed, and the canonical link when the page turned out to be
/// a thread. Identity, dates, `replicated_at`, `summary`, and every other extra key stay, and so
/// do `hidden`, `labels`, `authors`, and `first_seen`, which hand edits own.
fn upgrade_front(existing: FrontMatter, planned: FrontMatter) -> FrontMatter {
    let mut front = FrontMatter {
        link: planned.link,
        content: planned.content,
        html: planned.html,
        html_truncated: planned.html_truncated,
        preview: planned.preview,
        images: planned.images,
        ..existing
    };
    for key in PAGE_DERIVED_EXTRA_KEYS {
        if let Some(value) = planned.extra.get(key) {
            front.extra.insert(key.to_string(), value.clone());
        }
    }
    front
}

#[cfg(test)]
mod tests {
    use super::super::index::index_archive;
    use super::super::plan::plan;
    use super::super::tests::{options, source};
    use super::super::{ArticleFailures, FetchOneContext, Options, StatePolicy, fetch_one};
    use super::*;
    use crate::http;
    use crate::store::Outcome;
    use chrono::{TimeZone, Utc};
    use httpmock::prelude::*;
    use std::collections::BTreeMap;
    use std::fs;
    use tokio::sync::OnceCell;
    use url::Url;

    #[tokio::test]
    async fn archived_loading_placeholders_fall_back_to_the_summary_and_upgrade_later() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.path("/feed");
                then.status(304);
            })
            .await;
        // harnesstax.github.io: the server HTML only says `loading…` until js/post.js runs.
        let shell = server
            .mock_async(|when, then| {
                when.path("/post");
                then.status(200)
                    .header("content-type", "text/html; charset=utf-8")
                    .body(
                        r#"<html><head><meta charset="utf-8"><title>HarnessTax</title></head><body><header></header><div id="tab-blog"><div id="blog-root" aria-busy="true"><p class="loading">loading…</p></div></div></body></html>"#,
                    );
            })
            .await;
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(root.path()));
        let first_seen = "2026-09-17T08:11:04Z".parse().unwrap();
        let front = FrontMatter {
            title: "HarnessTax".into(),
            source: "blog".into(),
            link: server.url("/post"),
            first_seen,
            summary: Some("Article URL: https://harnesstax.github.io/ Points: 111".into()),
            content: ContentKind::Extracted,
            html: Some("harnesstax.html".into()),
            ..Default::default()
        };
        store
            .write_item(NewItem {
                dir: "items/blog",
                stem: "harnesstax",
                front: &front,
                body: "loading…\n",
                html: Some("<div id=\"tab-blog\"><p>loading…</p></div>"),
                preview: None,
                images: &[],
            })
            .unwrap();
        let configured = Source {
            previews: false,
            engine: Engine::Feed {
                url: Url::parse(&server.url("/feed")).unwrap(),
            },
            ..source()
        };
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let run = || {
            let store = &store;
            let configured = &configured;
            let client = &client;
            let root = root.path();
            let cache = cache.path();
            async move {
                let (_, archive) = index_archive(store.items().unwrap());
                let test_options = Options {
                    existing_paths: Arc::new(OnceCell::new_with(Some(archive))),
                    ..options()
                };
                fetch_one(
                    configured,
                    FetchOneContext {
                        store,
                        store_root: root,
                        client,
                        cache_dir: cache,
                        options: &test_options,
                        article_failures: &ArticleFailures::default(),
                        state_policy: StatePolicy::DevCache,
                    },
                )
                .await
                .unwrap();
            }
        };
        // Still a shell: the summary replaces the placeholder and the retry is remembered.
        run().await;
        shell.assert_calls_async(1).await;
        let fallen_back = store.read_item("items/blog/harnesstax").unwrap();
        assert_eq!(fallen_back.front.content, ContentKind::Feed);
        assert_eq!(
            fallen_back.body,
            "Article URL: https://harnesstax.github.io/ Points: 111"
        );
        assert_eq!(fallen_back.front.html, None);
        assert_eq!(fallen_back.front.first_seen, first_seen);
        let marker = CaptureRetries::new(cache.path()).path(&server.url("/post"));
        assert!(marker.exists());
        // A day later the page renders on the server: the article is captured in place.
        shell.delete_async().await;
        server.mock_async(|when, then| {
            when.path("/post");
            then.status(200).header("content-type", "text/html").body(
                "<html><head><title>HarnessTax</title></head><body><article><h1>HarnessTax</h1><p>We evaluate twenty-one model and harness pairs spanning seven models and three harnesses on two benchmarks.</p><p>A second paragraph keeps the extraction meaningful and well above the readability thresholds used for short pages.</p></article></body></html>",
            );
        }).await;
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(25 * 3600);
        std::fs::File::options()
            .write(true)
            .open(&marker)
            .unwrap()
            .set_modified(old)
            .unwrap();
        run().await;
        let upgraded = store.read_item("items/blog/harnesstax").unwrap();
        assert_eq!(upgraded.front.content, ContentKind::Extracted);
        assert!(
            upgraded.body.contains("twenty-one model and harness pairs"),
            "{}",
            upgraded.body
        );
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn feed_only_captures_are_retried_daily_and_upgraded_once_the_page_is_available() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.path("/feed");
                then.status(304);
            })
            .await;
        let denied = server
            .mock_async(|when, then| {
                when.path("/post");
                then.status(403);
            })
            .await;
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(root.path()));
        let first_seen = "2026-07-16T12:00:00Z".parse().unwrap();
        let front = FrontMatter {
            title: "Why teens deserve safe AI".into(),
            source: "blog".into(),
            link: server.url("/post"),
            first_seen,
            labels: vec!["safety".into()],
            summary: Some("Feed summary only.".into()),
            content: ContentKind::Feed,
            hidden: true,
            ..Default::default()
        };
        store
            .write_item(NewItem {
                dir: "items/blog",
                stem: "teens",
                front: &front,
                body: "Feed summary only.\n",
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let configured = Source {
            previews: false,
            engine: Engine::Feed {
                url: Url::parse(&server.url("/feed")).unwrap(),
            },
            ..source()
        };
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let run = |expected_added: usize| {
            let store = &store;
            let configured = &configured;
            let client = &client;
            let root = root.path();
            let cache = cache.path();
            async move {
                let (_, archive) = index_archive(store.items().unwrap());
                let test_options = Options {
                    existing_paths: Arc::new(OnceCell::new_with(Some(archive))),
                    ..options()
                };
                let report = fetch_one(
                    configured,
                    FetchOneContext {
                        store,
                        store_root: root,
                        client,
                        cache_dir: cache,
                        options: &test_options,
                        article_failures: &ArticleFailures::default(),
                        state_policy: StatePolicy::DevCache,
                    },
                )
                .await
                .unwrap();
                assert_eq!(report.added, expected_added);
            }
        };
        // Unavailable: the feed copy stays, the failure is remembered, and the next run waits.
        run(0).await;
        run(0).await;
        denied.assert_calls_async(1).await;
        assert_eq!(
            store.read_item("items/blog/teens").unwrap().body,
            "Feed summary only.\n"
        );
        denied.delete_async().await;
        let available = server.mock_async(|when, then| {
            when.path("/post");
            then.status(200).header("content-type", "text/html").body(
                "<html><head><title>Why teens deserve safe AI</title></head><body><article><h1>Why teens deserve safe AI</h1><p>The complete article explains age-appropriate protections, learning tools, and parental controls in depth, with every paragraph the publisher wrote.</p><p>A second paragraph keeps the extraction meaningful and well above the readability thresholds used for short pages.</p></article></body></html>",
            );
        }).await;
        // Still backed off: nothing is requested until a day has passed.
        run(0).await;
        available.assert_calls_async(0).await;
        let marker = CaptureRetries::new(cache.path()).path(&server.url("/post"));
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(25 * 3600);
        std::fs::File::options()
            .write(true)
            .open(&marker)
            .unwrap()
            .set_modified(old)
            .unwrap();
        run(1).await;
        available.assert_calls_async(1).await;
        let upgraded = store.read_item("items/blog/teens").unwrap();
        assert_eq!(upgraded.front.content, ContentKind::Extracted);
        assert!(
            upgraded.body.contains("age-appropriate protections"),
            "{}",
            upgraded.body
        );
        assert_eq!(upgraded.front.first_seen, first_seen);
        assert_eq!(upgraded.front.labels, vec!["safety".to_string()]);
        assert!(upgraded.front.html.is_some());
        assert!(upgraded.front.hidden, "a hand-hidden capture stays hidden");
        assert_eq!(upgraded.front.replicated_at, None);
        assert_eq!(
            upgraded.front.summary.as_deref(),
            Some("Feed summary only.")
        );
        assert!(!marker.exists());
        run(0).await;
        available.assert_calls_async(1).await;
    }

    #[tokio::test]
    async fn retained_image_repairs_recover_missing_previews_without_losing_other_articles() {
        crate::http::install_crypto_provider();
        for previews in [true, false] {
            let server = MockServer::start_async().await;
            server
                .mock_async(|when, then| {
                    when.path("/feed");
                    then.status(304);
                })
                .await;
            let mut encoded = std::io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgb8(40, 30)
                .write_to(&mut encoded, image::ImageFormat::Png)
                .unwrap();
            let bytes = encoded.into_inner();
            let unavailable_preview_source = server
                .mock_async(|when, then| {
                    when.path("/preview-only.png");
                    then.status(404);
                })
                .await;
            server
                .mock_async(|when, then| {
                    when.path_includes(".png");
                    then.status(200).body(bytes.clone());
                })
                .await;
            let root = tempfile::tempdir().unwrap();
            let cache = tempfile::tempdir().unwrap();
            let store = Arc::new(Store::open(root.path()));
            let original_preview = preview::thumbnail(&bytes, None).unwrap();
            for name in ["healthy", "recover", "preview-only", "unrelated"] {
                let asset = media::prepare_asset(
                    &media::Candidate {
                        url: Url::parse(&server.url(format!("/{name}.png"))).unwrap(),
                        alt: None,
                    },
                    bytes.clone(),
                    &media::MediaLimits::default(),
                )
                .unwrap();
                let front = FrontMatter {
                    title: name.into(),
                    source: "blog".into(),
                    link: server.url(format!("/{name}")),
                    html: Some(format!("{name}.html")),
                    preview: Some(original_preview.metadata(name)),
                    images: vec![asset.metadata(name)],
                    ..Default::default()
                };
                store
                    .write_item(NewItem {
                        dir: "items/blog",
                        stem: name,
                        front: &front,
                        body: &format!("Preserved {name}.\n\n![Image]({})", asset.source_url),
                        html: Some("<p>Original HTML.</p>"),
                        preview: Some(&original_preview.bytes),
                        images: &[asset],
                    })
                    .unwrap();
                if matches!(name, "recover" | "preview-only") {
                    fs::remove_file(
                        root.path()
                            .join("items/blog")
                            .join(&front.preview.as_ref().unwrap().file),
                    )
                    .unwrap();
                }
                if matches!(name, "recover" | "unrelated") {
                    fs::remove_file(
                        root.path()
                            .join("items/blog")
                            .join(&front.images[0].original.file),
                    )
                    .unwrap();
                }
            }
            let original = store.items().unwrap();
            let healthy = root.path().join("items/blog/healthy.md");
            let healthy_modified = fs::metadata(&healthy).unwrap().modified().unwrap();
            let configured = Source {
                previews,
                engine: Engine::Feed {
                    url: Url::parse(&server.url("/feed")).unwrap(),
                },
                ..source()
            };
            let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
            for added in [3, 0] {
                let (_, archive) = index_archive(store.items().unwrap());
                let test_options = Options {
                    existing_paths: Arc::new(OnceCell::new_with(Some(archive))),
                    ..options()
                };
                let report = fetch_one(
                    &configured,
                    FetchOneContext {
                        store: &store,
                        store_root: root.path(),
                        client: &client,
                        cache_dir: cache.path(),
                        options: &test_options,
                        article_failures: &ArticleFailures::default(),
                        state_policy: StatePolicy::DevCache,
                    },
                )
                .await
                .unwrap();
                assert_eq!(report.added, added);
                assert_eq!(report.outcome, Outcome::Ok);
            }
            for previous in &original {
                let repaired = store.read_item(&previous.path).unwrap();
                assert_eq!(repaired.body, previous.body);
                assert_eq!(
                    store.read_html(&repaired).unwrap().as_deref(),
                    Some("<p>Original HTML.</p>")
                );
                assert!(
                    repaired
                        .front
                        .images
                        .iter()
                        .all(|image| store.image_files_present(&repaired.path, image))
                );
                if previews || matches!(repaired.front.title.as_str(), "healthy" | "unrelated") {
                    assert_eq!(
                        store.read_preview(&repaired).unwrap(),
                        Some(original_preview.bytes.clone())
                    );
                } else {
                    assert!(
                        repaired.front.preview.is_none(),
                        "unavailable pointer should not block media fallback"
                    );
                }
            }
            assert_eq!(
                fs::metadata(healthy).unwrap().modified().unwrap(),
                healthy_modified
            );
            unavailable_preview_source.assert_calls_async(0).await;
            let healthy_item = store.read_item("items/blog/healthy").unwrap();
            let file = root
                .path()
                .join("items/blog")
                .join(&healthy_item.front.preview.as_ref().unwrap().file);
            fs::remove_file(&file).unwrap();
            fs::create_dir(&file).unwrap();
            let (_, archive) = index_archive(store.items().unwrap());
            let test_options = Options {
                existing_paths: Arc::new(OnceCell::new_with(Some(archive))),
                ..options()
            };
            let error = repair_archived_images(&configured, &store, &test_options, None)
                .await
                .unwrap_err();
            assert!(
                format!("{error:#}")
                    .replace('\\', "/")
                    .contains("items/blog/healthy"),
                "non-NotFound preview errors retain the article context: {error:#}"
            );
        }
    }

    #[tokio::test]
    async fn unchanged_feed_repairs_all_missing_body_images_without_refetching_good_files() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        let feed = server
            .mock_async(|when, then| {
                when.path("/feed");
                then.status(304);
            })
            .await;
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let bytes = encoded.into_inner();
        let good = server
            .mock_async(|when, then| {
                when.path("/good.png");
                then.status(200).body(bytes.clone());
            })
            .await;
        let missing = server
            .mock_async(|when, then| {
                when.path_includes("/missing-");
                then.status(200).body(bytes.clone());
            })
            .await;
        let inaccessible = server
            .mock_async(|when, then| {
                when.path("/unavailable.png");
                then.status(404);
            })
            .await;
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(root.path()).with_image_cache(cache.path()));
        let original = media::prepare_asset(
            &media::Candidate {
                url: Url::parse(&server.url("/good.png")).unwrap(),
                alt: None,
            },
            bytes,
            &media::MediaLimits::default(),
        )
        .unwrap();
        let mut body = format!("Original body.\n\n![Good]({})\n", server.url("/good.png"));
        for index in 0..14 {
            body.push_str(&format!(
                "\n![Figure {index}]({})\n",
                server.url(format!("/missing-{index}.png"))
            ));
        }
        body.push_str(&format!(
            "\n![Unavailable]({})\n",
            server.url("/unavailable.png")
        ));
        let front = FrontMatter {
            title: "Retained article outside feed".into(),
            html: Some("story.html".into()),
            link: server.url("/article"),
            source: "blog".into(),
            images: vec![original.metadata("story")],
            ..Default::default()
        };
        store
            .write_item(NewItem {
                dir: "items/blog",
                stem: "story",
                front: &front,
                body: &body,
                html: Some("<article>Preserved HTML companion.</article>"),
                preview: None,
                images: &[original],
            })
            .unwrap();
        let original_file = root
            .path()
            .join("items/blog")
            .join(&front.images[0].original.file);
        let original_modified = fs::metadata(&original_file).unwrap().modified().unwrap();
        let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let configured = Source {
            engine: Engine::Feed {
                url: Url::parse(&server.url("/feed")).unwrap(),
            },
            ..source()
        };
        let failures = ArticleFailures::default();
        for expected in [1, 0] {
            let (_, archive) = index_archive(store.items().unwrap());
            let test_options = Options {
                existing_paths: Arc::new(OnceCell::new_with(Some(archive))),
                media_fetcher: Arc::new(
                    media::Fetcher::new(
                        &crate::config::FetchConfig::default(),
                        media::MediaLimits::default(),
                    )
                    .unwrap()
                    .with_cache(cache.path()),
                ),
                ..options()
            };
            let report = fetch_one(
                &configured,
                FetchOneContext {
                    store: &store,
                    store_root: root.path(),
                    client: &client,
                    cache_dir: cache.path(),
                    options: &test_options,
                    article_failures: &failures,
                    state_policy: StatePolicy::PersistentBranch,
                },
            )
            .await
            .unwrap();
            assert_eq!(report.added, expected);
            assert_eq!(report.unchanged, expected == 0);
        }
        let repaired = store.read_item("items/blog/story").unwrap();
        assert_eq!(repaired.body, body);
        assert_eq!(repaired.front.images.len(), 15);
        assert_eq!(
            store.read_html(&repaired).unwrap().as_deref(),
            Some("<article>Preserved HTML companion.</article>")
        );
        assert_eq!(
            fs::metadata(original_file).unwrap().modified().unwrap(),
            original_modified
        );
        assert_eq!(good.calls_async().await, 0);
        assert_eq!(missing.calls_async().await, 14);
        assert_eq!(inaccessible.calls_async().await, 1);
        assert_eq!(feed.calls_async().await, 2);
    }

    #[test]
    fn reprocess_re_derives_stored_bodies_from_retained_html() {
        // An item captured before a cleanup rule existed: blog.google's audio player survived into
        // the stored body, and the source has long since stopped listing the article.
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(root.path()));
        let html = "<div data-component=\"uni-audio-player-tts\"><p><audio title=\"Listen\"><source src=\"https://cdn.example/a.mp3\" type=\"audio/mpeg\"><p>Your browser does not support the audio element.</p></audio></p><div><p>Listen to article</p><p>[[duration]] minutes</p></div></div><p>Today, we are launching the app.</p>";
        let stale = "Your browser does not support the audio element.\n\nListen to article\n\n\\[\\[duration\\]\\] minutes\n\nToday, we are launching the app.\n";
        let front = FrontMatter {
            source: "blog".into(),
            title: "Launch".into(),
            link: "https://blog.example/launch".into(),
            content: ContentKind::Extracted,
            html: Some("launch.html".into()),
            ..Default::default()
        };
        store
            .write_item(NewItem {
                dir: "items/blog",
                stem: "launch",
                front: &front,
                body: stale,
                html: Some(html),
                preview: None,
                images: &[],
            })
            .unwrap();

        let configured = source();
        let (_, archive) = index_archive(store.items().unwrap());
        let idle = Options {
            existing_paths: Arc::new(OnceCell::new_with(Some(archive))),
            ..options()
        };
        // Without the flag nothing is touched, however stale the body is.
        assert_eq!(
            reprocess_stored_bodies(&configured, &store, &idle, None).unwrap(),
            0
        );
        assert_eq!(store.read_item("items/blog/launch").unwrap().body, stale);

        let (_, archive) = index_archive(store.items().unwrap());
        let reprocessing = Options {
            existing_paths: Arc::new(OnceCell::new_with(Some(archive))),
            reprocess: true,
            ..options()
        };
        assert_eq!(
            reprocess_stored_bodies(&configured, &store, &reprocessing, None).unwrap(),
            1
        );
        let repaired = store.read_item("items/blog/launch").unwrap();
        assert_eq!(repaired.body, "Today, we are launching the app.\n");
        // The retained HTML and the front matter are left exactly as they were.
        assert_eq!(store.read_html(&repaired).unwrap().as_deref(), Some(html));
        assert_eq!(repaired.front.title, front.title);
        assert_eq!(repaired.front.html.as_deref(), Some("launch.html"));

        // A second run has nothing left to do.
        let (_, archive) = index_archive(store.items().unwrap());
        let again = Options {
            existing_paths: Arc::new(OnceCell::new_with(Some(archive))),
            reprocess: true,
            ..options()
        };
        assert_eq!(
            reprocess_stored_bodies(&configured, &store, &again, None).unwrap(),
            0
        );
    }

    #[test]
    fn reprocess_never_shortens_an_item_whose_retained_html_was_truncated() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(root.path()));
        let body = "The complete article body, longer than what was retained.\n";
        let front = FrontMatter {
            source: "blog".into(),
            title: "Long".into(),
            link: "https://blog.example/long".into(),
            content: ContentKind::Extracted,
            html: Some("long.html".into()),
            html_truncated: true,
            ..Default::default()
        };
        store
            .write_item(NewItem {
                dir: "items/blog",
                stem: "long",
                front: &front,
                body,
                html: Some("<p>The complete article"),
                preview: None,
                images: &[],
            })
            .unwrap();
        let (_, archive) = index_archive(store.items().unwrap());
        let reprocessing = Options {
            existing_paths: Arc::new(OnceCell::new_with(Some(archive))),
            reprocess: true,
            ..options()
        };
        assert_eq!(
            reprocess_stored_bodies(&source(), &store, &reprocessing, None).unwrap(),
            0
        );
        assert_eq!(store.read_item("items/blog/long").unwrap().body, body);
    }

    #[tokio::test]
    async fn unchanged_feed_backfills_cached_recording_duration_without_rewriting_content() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        let feed = server
            .mock_async(|when, then| {
                when.path("/feed");
                then.status(304);
            })
            .await;
        let page = server
            .mock_async(|when, then| {
                when.path("/episode");
                then.status(500);
            })
            .await;
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(root.path()));
        let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let configured = Source {
            engine: Engine::Feed {
                url: Url::parse(&server.url("/feed")).unwrap(),
            },
            ..source()
        };
        let raw = RawItem {
            title: "Retained recording".into(),
            link: server.url("/episode"),
            content_html: Some("<p>Keep this original article.</p>".into()),
            extra: [("audio_url".into(), server.url("/recording.mp3").into())].into(),
            ..Default::default()
        };
        let mut test_options = options();
        let planned = plan(&raw, &configured, &test_options, ContentKind::Feed);
        store
            .write_item(NewItem {
                dir: &planned.dir,
                stem: &planned.stem,
                front: &planned.front,
                body: &planned.body,
                html: planned.html.as_deref(),
                preview: None,
                images: &[],
            })
            .unwrap();
        let path = format!("{}/{}", planned.dir, planned.stem);
        let url = Url::parse(&raw.link).unwrap();
        crate::cache::ArticleCache::new(cache.path()).store(&url, &[], &http::Body {
            final_url: url.clone(), content_type: Some("text/html".into()), etag: None, last_modified: None,
            bytes: format!(r#"<script type="application/ld+json">{{"@type":"AudioObject","contentUrl":"{}","duration":"PT27M51S"}}</script>"#, raw.extra["audio_url"].as_str().unwrap()).into_bytes(),
        }).unwrap();
        test_options.existing_paths = Arc::new(OnceCell::new_with(Some(
            index_archive(store.items().unwrap()).1,
        )));
        let failures = ArticleFailures::default();
        let context = FetchOneContext {
            store: &store,
            store_root: root.path(),
            client: &client,
            cache_dir: cache.path(),
            options: &test_options,
            article_failures: &failures,
            state_policy: StatePolicy::PersistentBranch,
        };
        let html = fs::read(root.path().join(format!("{path}.html"))).unwrap();
        assert_eq!(fetch_one(&configured, context).await.unwrap().added, 1);
        let retained = store.read_item(&path).unwrap();
        assert_eq!(
            retained.front.extra["duration_seconds"].as_u64(),
            Some(1671)
        );
        assert_eq!(retained.body, planned.body);
        assert_eq!(
            fs::read(root.path().join(format!("{path}.html"))).unwrap(),
            html
        );
        let snapshots: Vec<_> = [
            format!("{path}.md"),
            format!("sources/{}/state.toml", configured.slug),
        ]
        .into_iter()
        .map(|path| {
            let path = root.path().join(path);
            let bytes = fs::read(&path).unwrap();
            let modified = fs::metadata(&path).unwrap().modified().unwrap();
            (path, bytes, modified)
        })
        .collect();
        let unchanged = fetch_one(&configured, context).await.unwrap();
        assert!(unchanged.unchanged);
        assert_eq!(unchanged.added, 0);
        for (path, bytes, modified) in snapshots {
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
        }
        // The feed-only capture is retried once (with the client's transient retries) on the
        // first run, fails, and is then backed off; the duration backfill itself never refetches.
        page.assert_calls_async(3).await;
        feed.assert_calls_async(2).await;
    }

    #[tokio::test]
    async fn spotify_reconciliation_preserves_items_is_transactional_and_repeats_without_writes() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        server.mock_async(|when, then| {
            when.path("/feed");
            then.status(200).header("content-type", "application/rss+xml").body(r#"<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd"><channel><title>Publisher podcast</title><link>https://publisher.example/</link><description>Podcast</description><item><guid>canonical-guid</guid><link>https://publisher.example/episode</link><title>AN Episode!</title><pubDate>Tue, 01 Sep 2026 12:00:00 GMT</pubDate><description>Different publisher body</description><enclosure url="https://cdn.example/episode.mp3" type="audio/mpeg" length="1200"/><itunes:duration>1:00:01</itunes:duration></item></channel></rss>"#);
        }).await;
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(root.path()));
        let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let configured = Source {
            public_url: Some("https://open.spotify.com/show/1sz1NhoHqbpXbzNlpOnFoz".into()),
            content: ContentMode::Light,
            images: false,
            engine: Engine::Feed {
                url: Url::parse(&server.url("/feed")).unwrap(),
            },
            ..source()
        };
        let raw = RawItem {
            title: "An Episode!".into(),
            link: "https://open.spotify.com/episode/old-id".into(),
            id: Some("spotify-old-id".into()),
            published: Some("2026-09-01T00:00:00Z".parse().unwrap()),
            content_html: Some("<p>Original archived body.</p>".into()),
            extra: [("custom".into(), "kept".into())].into(),
            ..Default::default()
        };
        let test_options = options();
        let mut planned = plan(&raw, &configured, &test_options, ContentKind::Feed);
        planned.front.labels = vec!["retained-label".into()];
        planned.front.hidden = true;
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4, 4)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let preview = preview::thumbnail(png.get_ref(), Some("Retained preview".into())).unwrap();
        planned.front.preview = Some(preview.metadata(&planned.stem));
        store
            .write_item(NewItem {
                dir: &planned.dir,
                stem: &planned.stem,
                front: &planned.front,
                body: &planned.body,
                html: planned.html.as_deref(),
                preview: Some(&preview.bytes),
                images: &[],
            })
            .unwrap();
        store
            .append_seen(&configured.slug, &dedupe_keys(&raw), test_options.now)
            .unwrap();
        let path = format!("{}/{}", planned.dir, planned.stem);
        let before = store.read_item(&path).unwrap();
        let md_path = root.path().join(format!("{path}.md"));
        let seen_path = root.path().join("sources/blog/seen.txt");
        let original_md = fs::read(&md_path).unwrap();
        let original_seen = fs::read(&seen_path).unwrap();
        let original_html = fs::read(root.path().join(format!("{path}.html"))).unwrap();
        let preview_path = root
            .path()
            .join(&planned.dir)
            .join(&before.front.preview.as_ref().unwrap().file);
        let original_preview = fs::read(&preview_path).unwrap();
        let failures = ArticleFailures::default();
        let run = |options| {
            fetch_one(
                &configured,
                FetchOneContext {
                    store: &store,
                    store_root: root.path(),
                    client: &client,
                    cache_dir: cache.path(),
                    options,
                    article_failures: &failures,
                    state_policy: StatePolicy::PersistentBranch,
                },
            )
        };
        let dry_options = Options {
            dry_run: true,
            ..test_options.clone()
        };
        assert_eq!(run(&dry_options).await.unwrap().added, 1);
        assert_eq!(fs::read(&md_path).unwrap(), original_md);
        assert_eq!(fs::read(&seen_path).unwrap(), original_seen);
        assert!(!root.path().join("sources/blog/state.toml").exists());
        // Failure after aliases were appended must restore both the existing item and seen keys.
        let state_path = root.path().join("sources/blog/state.toml");
        fs::create_dir(&state_path).unwrap();
        let canonical_audio = RawItem {
            id: Some("canonical-guid".into()),
            link: "https://publisher.example/episode".into(),
            title: "AN Episode!".into(),
            published: Some("2026-09-01T12:00:00Z".parse().unwrap()),
            extra: [("audio_url".into(), "https://cdn.example/episode.mp3".into())].into(),
            ..Default::default()
        };
        let mut transaction = SourceTransaction::new(root.path()).unwrap();
        let (_, aliases) = reconcile_podcast(
            &canonical_audio,
            &path,
            &configured,
            &store,
            &test_options,
            Some(&mut transaction),
            &store.seen("blog").unwrap(),
        )
        .unwrap()
        .unwrap();
        transaction.track_seen("blog").unwrap();
        store
            .append_seen("blog", &aliases, test_options.now)
            .unwrap();
        assert_ne!(fs::read(&md_path).unwrap(), original_md);
        assert_ne!(fs::read(&seen_path).unwrap(), original_seen);
        assert!(transaction.track_state("blog").is_err());
        transaction.rollback().unwrap();
        assert_eq!(fs::read(&md_path).unwrap(), original_md);
        assert_eq!(fs::read(&seen_path).unwrap(), original_seen);
        fs::remove_dir(&state_path).unwrap();
        assert_eq!(run(&test_options).await.unwrap().added, 1);
        assert_eq!(store.items().unwrap().len(), 1);
        let mut expected = before.clone();
        expected
            .front
            .extra
            .insert("audio_url".into(), "https://cdn.example/episode.mp3".into());
        expected
            .front
            .extra
            .insert("duration_seconds".into(), 3601.into());
        assert_eq!(store.read_item(&path).unwrap(), expected);
        assert_eq!(
            fs::read(root.path().join(format!("{path}.html"))).unwrap(),
            original_html
        );
        assert_eq!(fs::read(&preview_path).unwrap(), original_preview);
        let canonical = RawItem {
            id: Some("canonical-guid".into()),
            link: "https://publisher.example/episode".into(),
            title: "AN Episode!".into(),
            published: Some("2026-09-01T12:00:00Z".parse().unwrap()),
            ..Default::default()
        };
        assert!(
            dedupe_keys(&canonical)
                .iter()
                .all(|key| store.seen("blog").unwrap().contains(key))
        );
        // A corrected duration must update even when the enclosure already matches.
        let corrected = RawItem {
            extra: [
                ("audio_url".into(), "https://cdn.example/episode.mp3".into()),
                ("duration_seconds".into(), 1671.into()),
            ]
            .into(),
            ..canonical_audio.clone()
        };
        let reconciled = reconcile_podcast(
            &corrected,
            &path,
            &configured,
            &store,
            &test_options,
            None,
            &store.seen("blog").unwrap(),
        )
        .unwrap()
        .unwrap();
        assert!(reconciled.0);
        assert_eq!(
            store.read_item(&path).unwrap().front.extra["duration_seconds"].as_u64(),
            Some(1671)
        );
        assert_eq!(
            fs::read(root.path().join(format!("{path}.html"))).unwrap(),
            original_html
        );
        assert!(
            !reconcile_podcast(
                &corrected,
                &path,
                &configured,
                &store,
                &test_options,
                None,
                &store.seen("blog").unwrap()
            )
            .unwrap()
            .unwrap()
            .0
        );
        // Restore the authoritative feed value before checking the feed no-op.
        reconcile_podcast(
            &RawItem {
                extra: [
                    ("audio_url".into(), "https://cdn.example/episode.mp3".into()),
                    ("duration_seconds".into(), 3601.into()),
                ]
                .into(),
                ..corrected
            },
            &path,
            &configured,
            &store,
            &test_options,
            None,
            &store.seen("blog").unwrap(),
        )
        .unwrap();
        let snapshots: Vec<_> = [&md_path, &seen_path, &state_path]
            .into_iter()
            .map(|file| {
                (
                    file,
                    fs::read(file).unwrap(),
                    fs::metadata(file).unwrap().modified().unwrap(),
                )
            })
            .collect();
        let report = run(&test_options).await.unwrap();
        assert_eq!(report.added, 0);
        assert!(report.unchanged);
        for (file, bytes, modified) in snapshots {
            assert_eq!(fs::read(file).unwrap(), bytes);
            assert_eq!(fs::metadata(file).unwrap().modified().unwrap(), modified);
        }
    }

    #[test]
    fn upgrade_front_replaces_only_what_the_original_page_supplies() {
        use serde_yaml_ng::Value;

        let existing_first_seen = Utc.with_ymd_and_hms(2026, 7, 16, 12, 0, 0).unwrap();
        let replicated_at = Utc.with_ymd_and_hms(2026, 7, 17, 8, 0, 0).unwrap();
        let published = Utc.with_ymd_and_hms(2026, 7, 15, 9, 0, 0).unwrap();
        let updated = Utc.with_ymd_and_hms(2026, 7, 15, 10, 0, 0).unwrap();
        let later = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
        let existing = FrontMatter {
            title: "Kept title".into(),
            link: "https://twitter.com/alice/status/1".into(),
            source: "blog".into(),
            published: Some(published),
            updated: Some(updated),
            first_seen: existing_first_seen,
            replicated_at: Some(replicated_at),
            authors: vec!["Hand-added author".into()],
            labels: vec!["hand-label".into()],
            summary: Some("Feed summary".into()),
            content: ContentKind::Feed,
            html: None,
            preview: None,
            images: Vec::new(),
            html_truncated: false,
            extra: BTreeMap::from([
                ("via".to_string(), Value::from("https://news.example/via")),
                (
                    "audio_url".to_string(),
                    Value::from("https://cdn.example/a.mp3"),
                ),
                ("duration_seconds".to_string(), Value::from(10u64)),
            ]),
            hidden: true,
        };
        let preview = crate::model::Preview {
            file: "post.jpg".into(),
            width: 640,
            height: 400,
            alt: None,
            color: None,
        };
        let image = crate::model::ArticleImage {
            source: "https://blog.example/hero.png".into(),
            original: crate::model::ImageFile {
                file: "post-1.png".into(),
                width: 800,
                height: 600,
            },
            variants: Vec::new(),
            color: None,
        };
        let planned = FrontMatter {
            title: "Planned title".into(),
            link: "https://x.com/alice/status/1".into(),
            source: "blog".into(),
            published: Some(later),
            updated: Some(later),
            first_seen: later,
            replicated_at: Some(later),
            authors: vec!["Feed author".into()],
            labels: vec!["feed-label".into()],
            summary: None,
            content: ContentKind::Extracted,
            html: Some("post.html".into()),
            preview: Some(preview.clone()),
            images: vec![image.clone()],
            html_truncated: true,
            extra: BTreeMap::from([
                ("duration_seconds".to_string(), Value::from(1234u64)),
                (
                    crate::site::interactive::METADATA_KEY.to_string(),
                    Value::from(true),
                ),
                ("via".to_string(), Value::from("https://other.example/")),
                ("stray".to_string(), Value::from("value")),
            ]),
            hidden: false,
        };

        let upgraded = upgrade_front(existing.clone(), planned.clone());
        // Kept from the archive: identity, dates, hand-owned fields, and the item's own extras.
        assert_eq!(upgraded.title, "Kept title");
        assert_eq!(upgraded.source, "blog");
        assert_eq!(upgraded.published, Some(published));
        assert_eq!(upgraded.updated, Some(updated));
        assert_eq!(upgraded.first_seen, existing_first_seen);
        assert_eq!(upgraded.replicated_at, Some(replicated_at));
        assert_eq!(upgraded.authors, vec!["Hand-added author".to_string()]);
        assert_eq!(upgraded.labels, vec!["hand-label".to_string()]);
        assert_eq!(upgraded.summary.as_deref(), Some("Feed summary"));
        assert!(upgraded.hidden);
        assert_eq!(
            upgraded.extra["via"],
            Value::from("https://news.example/via")
        );
        assert_eq!(
            upgraded.extra["audio_url"],
            Value::from("https://cdn.example/a.mp3")
        );
        assert!(!upgraded.extra.contains_key("stray"));
        // Taken from the page: the canonical thread link, content kind, HTML, companions, and the
        // extras the page revealed.
        assert_eq!(upgraded.link, "https://x.com/alice/status/1");
        assert_eq!(upgraded.content, ContentKind::Extracted);
        assert_eq!(upgraded.html.as_deref(), Some("post.html"));
        assert!(upgraded.html_truncated);
        assert_eq!(upgraded.preview, Some(preview));
        assert_eq!(upgraded.images, vec![image]);
        assert_eq!(upgraded.extra["duration_seconds"], Value::from(1234u64));
        assert_eq!(
            upgraded.extra[crate::site::interactive::METADATA_KEY],
            Value::from(true)
        );

        // A page that reveals no duration or interactivity leaves the archived values alone, and
        // a feed capture that was never replicated stays that way.
        let quiet = FrontMatter {
            extra: BTreeMap::new(),
            ..planned
        };
        let untouched = upgrade_front(
            FrontMatter {
                replicated_at: None,
                ..existing
            },
            quiet,
        );
        assert_eq!(untouched.extra["duration_seconds"], Value::from(10u64));
        assert!(
            !untouched
                .extra
                .contains_key(crate::site::interactive::METADATA_KEY)
        );
        assert_eq!(untouched.replicated_at, None);
        assert!(untouched.hidden);
    }
}
