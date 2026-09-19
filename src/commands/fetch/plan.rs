//! What an item becomes on disk, decided before anything is written, and the source-state
//! decisions that go with a fetch.

use std::path::{Component, Path};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};

use super::{Options, StatePolicy};
use crate::config::{Engine, Source};
use crate::content;
use crate::media;
use crate::model::{
    ContentKind, FrontMatter, Item, RawItem, file_stem, item_dir, normalize_labels,
};
use crate::sources;
use crate::store::{NewItem, SourceState, Store};

/// An item's files, decided before anything is written.
pub(super) struct Planned {
    pub(super) dir: String,
    pub(super) stem: String,
    pub(super) front: FrontMatter,
    pub(super) body: String,
    pub(super) html: Option<String>,
}

impl Planned {
    /// An archived item about to be rewritten in place: its front matter and body as stored, with
    /// `dir` and `stem` taken from `path` once it is known to name a file inside the source's own
    /// directory.
    pub(super) fn from_existing(item: Item, path: &str) -> Result<Self> {
        let mut planned = Self {
            dir: String::new(),
            stem: String::new(),
            front: item.front,
            body: item.body,
            html: None,
        };
        use_existing_path(&mut planned, path)?;
        Ok(planned)
    }
}

pub(super) async fn prepare_item(
    raw: RawItem,
    source: &Source,
    options: &Options,
    kind: ContentKind,
) -> Result<(RawItem, Planned)> {
    let permit = options
        .preparation_limit
        .clone()
        .acquire_owned()
        .await
        .context("article preparation was closed")?;
    let source = source.clone();
    let options = options.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let planned = plan(&raw, &source, &options, kind);
        (raw, planned)
    })
    .await
    .context("preparing article content")
}

/// Write an item off the runtime workers: validating the companions decodes every image master
/// and rendition again, which would otherwise stall the sources sharing the worker. Bounded like
/// preparation so a burst of image-heavy articles cannot exhaust the blocking pool either. The
/// caller tracks the item in its transaction first, so rollback ordering is unchanged.
pub(super) async fn persist_item(
    store: &Arc<Store>,
    options: &Options,
    planned: Planned,
    raw: RawItem,
) -> Result<()> {
    let permit = options
        .persist_limit
        .clone()
        .acquire_owned()
        .await
        .context("article persistence was closed")?;
    let store = Arc::clone(store);
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        store.write_item(NewItem {
            dir: &planned.dir,
            stem: &planned.stem,
            front: &planned.front,
            body: &planned.body,
            html: planned.html.as_deref(),
            preview: raw.preview.as_ref().map(|preview| preview.bytes.as_slice()),
            images: &raw.images,
        })
    })
    .await
    .context("persisting article content")?
}

pub(super) fn plan(
    raw: &RawItem,
    source: &Source,
    options: &Options,
    content_kind: ContentKind,
) -> Planned {
    // A future-dated entry would otherwise land in a directory that does not exist yet.
    let published = raw.published.map(|date| date.min(options.now));
    let updated = raw.updated.map(|date| date.min(options.now));
    let first_seen = raw
        .first_seen
        .map(|date| date.min(options.now))
        .unwrap_or(options.now);
    let date = published.or(updated).unwrap_or(first_seen);
    let base = url::Url::parse(&raw.link).ok();
    let body = match &raw.content_html {
        Some(html) => content::to_markdown(html, base.as_ref()),
        None => raw.summary.clone().unwrap_or_default(),
    };
    let body = content::strip_article_metadata(&body, &raw.title, published, &source.slug);
    let (html, truncated) = match &raw.content_html {
        Some(html) if options.html && source.html => {
            let (stored, truncated) = content::storage_html(html, options.html_max_bytes);
            (Some(stored), truncated)
        }
        _ => (None, false),
    };
    let front = FrontMatter {
        title: raw.title.clone(),
        link: raw.link.clone(),
        source: source.slug.clone(),
        published,
        updated,
        first_seen,
        replicated_at: raw.first_seen.map(|_| options.now),
        authors: raw.authors.clone(),
        labels: normalize_labels(source.labels.iter().chain(&raw.labels)),
        summary: raw.summary.clone().filter(|s| !s.trim().is_empty()),
        content: content_kind,
        html: None,
        preview: None,
        images: Vec::new(),
        html_truncated: truncated,
        extra: crate::sources::aggr::persistent_extra(raw),
        hidden: false,
    };
    Planned {
        dir: item_dir(&source.slug, date),
        stem: file_stem(date, &raw.title),
        front,
        body,
        html,
    }
}

pub(super) fn merge_image_metadata(
    metadata: &mut Vec<crate::model::ArticleImage>,
    images: &[media::Asset],
    stem: &str,
) {
    for image in images {
        let next = image.metadata(stem);
        if let Some(existing) = metadata
            .iter_mut()
            .find(|existing| existing.source == next.source)
        {
            *existing = next;
        } else {
            metadata.push(next);
        }
    }
}

pub(super) fn use_existing_path(planned: &mut Planned, existing_path: &str) -> Result<()> {
    let path = Path::new(existing_path);
    let expected = Path::new("items").join(&planned.front.source);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || !path.starts_with(&expected)
        || path == expected
    {
        bail!("invalid existing item path: {existing_path}");
    }
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("existing item path has no file name")?;
    let directory = path
        .parent()
        .and_then(Path::to_str)
        .context("existing item path has no directory")?;
    planned.dir = directory.replace('\\', "/");
    planned.stem = stem.to_string();
    Ok(())
}

fn keep_newest(items: &mut Vec<RawItem>, limit: usize) {
    items.sort_by_key(|item| std::cmp::Reverse(item.created_at()));
    items.truncate(limit);
}

pub(super) fn apply_first_import_limit(
    items: &mut Vec<RawItem>,
    engine: &Engine,
    feed_limit: usize,
) {
    // Aggr sources already apply their own optional limit. With no limit, importing the full
    // retained tree is what makes one instance a useful replica of another.
    if matches!(engine, Engine::Feed { .. }) {
        keep_newest(items, feed_limit);
    }
}

pub(super) fn source_request_state(state: &SourceState, refresh: bool) -> SourceState {
    if refresh {
        SourceState::default()
    } else {
        state.clone()
    }
}

pub(super) fn apply_validators(
    mut validators: sources::Validators,
    state: &mut crate::store::SourceState,
    source: &Source,
) {
    validators.resolved_url = if source.persist_endpoint {
        validators
            .resolved_url
            .and_then(|url| url::Url::parse(&url).ok())
            .map(|url| crate::config::public_url(&url, false))
    } else {
        None
    };
    validators.apply(state);
}

/// Whether a recorded upstream title or site URL changed: a publisher rename or move the site
/// would otherwise keep showing under the old name. Titles are compared with surrounding and
/// repeated whitespace collapsed, so a feed that reformats its `<title>` between runs cannot churn
/// the data branch; the site URL is already canonical. A value recorded for the first time is not
/// a change on its own: it is written with the source's first item, so a source that has only
/// ever found duplicates leaves no trace.
/// A language learned or changed for a source that already has state counts too: it is what
/// lets articles archived before the field existed carry their language on the next build. A
/// brand-new source records it silently with its first item.
pub(super) fn source_metadata_changed(previous: &SourceState, next: &SourceState) -> bool {
    fn renamed<T: PartialEq>(before: Option<T>, after: Option<T>) -> bool {
        before.is_some() && before != after
    }
    renamed(
        normalized_title(previous.title.as_deref()),
        normalized_title(next.title.as_deref()),
    ) || renamed(previous.site_url.as_deref(), next.site_url.as_deref())
        || (!previous.identity.is_empty() && previous.language != next.language)
}

fn normalized_title(title: Option<&str>) -> Option<String> {
    title
        .map(|title| title.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|title| !title.is_empty())
}

/// Whether `sources/<slug>/state.toml` is written this run. On the append-only branch a source
/// only earns a commit for something the site shows: new or repaired items, or a changed
/// upstream title or site URL. Validator-only changes are dropped so a run that finds nothing new
/// leaves no trace. Dev's private cache keeps validators current regardless.
pub(super) fn should_persist_state(
    added: usize,
    repaired: usize,
    metadata_changed: bool,
    policy: StatePolicy,
) -> bool {
    policy == StatePolicy::DevCache || added > 0 || repaired > 0 || metadata_changed
}

/// Replace the expanded fetch URL with its public form, then any credential it carries wherever
/// it appears. A redirect, feed discovery, or an HTTP layer quoting the request can put the
/// same username or token in a URL that is not byte-identical to the configured one, so the
/// substring pass is deliberately eager: over-redacting a message costs less than committing a
/// secret to `status.toml`.
pub(super) fn redact_source_secrets(source: &Source, message: &str) -> String {
    let Some(private) = source.engine.url() else {
        return message.to_string();
    };
    let replacement = source.public_url.as_deref().unwrap_or("[source URL]");
    let mut message = message.replace(private.as_str(), replacement);
    for secret in [private.password(), Some(private.username())]
        .into_iter()
        .flatten()
        .filter(|secret| !secret.is_empty())
    {
        message = message.replace(secret, "[redacted]");
    }
    message
}

#[cfg(test)]
mod tests {
    use super::super::sanitized_source_error;
    use super::super::tests::{options, source};
    use super::*;
    use crate::http;
    use chrono::{TimeZone, Utc};
    use httpmock::prelude::*;
    use std::time::{Duration, Instant};
    use url::Url;

    #[test]
    fn mirror_companion_locator_never_reaches_front_matter() {
        let mut raw = RawItem {
            title: "Mirrored".into(),
            link: "https://example.com/mirrored".into(),
            ..Default::default()
        };
        raw.extra.insert("kept".into(), "value".into());
        crate::sources::aggr::attach_companion_locator(
            &mut raw,
            Path::new("/private/cache"),
            "items/source/mirrored",
        )
        .unwrap();
        let planned = plan(&raw, &source(), &options(), ContentKind::Feed);
        assert_eq!(
            planned.front.extra,
            std::collections::BTreeMap::from([("kept".into(), "value".into())])
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn persisting_a_large_image_never_blocks_another_source_request() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        let other_feed = server
            .mock_async(|when, then| {
                when.method(GET).path("/other-feed");
                then.status(200).body("<rss/>");
            })
            .await;
        // Validation decodes the master and every rendition again; on the single runtime worker
        // that takes far longer than a loopback request.
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(2000, 1500, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 199) as u8])
        }))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
        let asset = media::prepare_asset(
            &media::Candidate {
                url: Url::parse(&server.url("/large.png")).unwrap(),
                alt: None,
            },
            bytes.into_inner(),
            &media::MediaLimits::default(),
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(root.path()));
        let raw = RawItem {
            title: "Large".into(),
            link: server.url("/large"),
            images: vec![asset],
            ..Default::default()
        };
        let test_options = options();
        let mut planned = plan(&raw, &source(), &test_options, ContentKind::Feed);
        merge_image_metadata(&mut planned.front.images, &raw.images, &planned.stem);
        let path = format!("{}/{}", planned.dir, planned.stem);
        let started = Instant::now();
        let writer = tokio::spawn({
            let (store, test_options) = (store.clone(), test_options.clone());
            async move {
                persist_item(&store, &test_options, planned, raw)
                    .await
                    .unwrap();
                started.elapsed()
            }
        });
        // Let the writer reach its blocking step on the only worker before measuring.
        tokio::task::yield_now().await;
        let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let url = Url::parse(&server.url("/other-feed")).unwrap();
        let response = tokio::time::timeout(
            Duration::from_secs(30),
            client.get(http::Request::get(&url)),
        )
        .await
        .expect("the request must not wait for the write")
        .unwrap();
        let requested = started.elapsed();
        assert!(matches!(response, http::Response::Ok(_)));
        let written = writer.await.unwrap();
        assert!(
            requested < written,
            "the request took {requested:?} because it waited for the {written:?} write"
        );
        other_feed.assert_calls_async(1).await;
        assert_eq!(store.read_item(&path).unwrap().front.images.len(), 1);
    }

    #[test]
    fn refresh_discards_fetch_validators_so_existing_items_can_be_reprocessed() {
        let remembered = crate::store::SourceState {
            identity: "blog".into(),
            resolved_url: Some("https://blog.example/feed.xml".into()),
            etag: Some("old-etag".into()),
            last_modified: Some("yesterday".into()),
            body_hash: Some("old-body".into()),
            ..Default::default()
        };

        assert_eq!(source_request_state(&remembered, false), remembered);
        assert_eq!(
            source_request_state(&remembered, true),
            crate::store::SourceState::default()
        );
    }

    #[test]
    fn plans_paths_from_the_published_date_and_clamps_the_future() {
        let raw = RawItem {
            title: "Hello, World!".into(),
            link: "https://blog.example/hello".into(),
            published: Some(Utc.with_ymd_and_hms(2026, 8, 30, 1, 0, 0).unwrap()),
            content_html: Some("<p>Hi <script>x()</script><b>there</b></p>".into()),
            ..Default::default()
        };
        let planned = plan(&raw, &source(), &options(), ContentKind::Feed);
        assert_eq!(planned.dir, "items/blog/2026/08");
        assert_eq!(planned.stem, "2026-08-30-hello-world");
        assert_eq!(planned.front.content, ContentKind::Feed);
        assert_eq!(planned.front.first_seen, options().now);
        assert!(planned.body.contains("**there**"), "{}", planned.body);
        assert!(!planned.body.contains("script"));
        let html = planned.html.unwrap();
        assert!(!html.contains("<script>"), "{html}");

        let future = RawItem {
            published: Some(Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap()),
            ..raw
        };
        let planned = plan(&future, &source(), &options(), ContentKind::None);
        assert_eq!(planned.dir, "items/blog/2026/09");
        assert_eq!(planned.front.published, Some(options().now));
    }

    #[test]
    fn plan_preserves_imported_first_seen_and_clamps_future_values() {
        let first_seen = Utc.with_ymd_and_hms(2024, 3, 2, 1, 0, 0).unwrap();
        let raw = RawItem {
            title: "Undated imported item".into(),
            link: "https://example.com/imported".into(),
            first_seen: Some(first_seen),
            ..Default::default()
        };
        let planned = plan(&raw, &source(), &options(), ContentKind::None);
        assert_eq!(planned.front.first_seen, first_seen);
        assert_eq!(planned.front.replicated_at, Some(options().now));
        assert_eq!(planned.front.published, None);
        assert_eq!(planned.dir, "items/blog/2024/03");

        let future = RawItem {
            first_seen: Some(Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap()),
            ..raw
        };
        let planned = plan(&future, &source(), &options(), ContentKind::None);
        assert_eq!(planned.front.first_seen, options().now);
        assert_eq!(planned.front.replicated_at, Some(options().now));
        assert_eq!(planned.dir, "items/blog/2026/09");
    }

    #[test]
    fn plan_drops_extracted_leading_publication_dates() {
        let published = Utc.with_ymd_and_hms(2026, 9, 2, 1, 0, 0).unwrap();
        let raw = RawItem {
            title: "Dated article".into(),
            link: "https://blog.example/dated".into(),
            published: Some(published),
            content_html: Some("<p>2nd September 2026</p><p>The actual opening.</p>".into()),
            ..Default::default()
        };
        assert_eq!(
            plan(&raw, &source(), &options(), ContentKind::Extracted).body,
            "The actual opening.\n"
        );
    }

    #[test]
    fn plan_cleans_boundary_metadata_for_feed_extracted_and_summary_content() {
        for kind in [ContentKind::Feed, ContentKind::Extracted, ContentKind::None] {
            let raw = RawItem {
                title: "Article".into(),
                link: "https://publisher.example/article".into(),
                content_html: (kind != ContentKind::None).then(|| {
                    "<p>Actual article.</p><p>[<a href=\"#comments\">0 comments</a>]</p>".into()
                }),
                summary: Some("Actual article.\n\n[0 comments]\n".into()),
                ..Default::default()
            };
            assert_eq!(
                plan(&raw, &source(), &options(), kind).body,
                "Actual article.\n"
            );
        }
    }

    #[test]
    fn titles_only_entries_keep_the_summary_as_body_and_no_html() {
        let raw = RawItem {
            title: "T".into(),
            link: "https://x/".into(),
            summary: Some("Just a summary".into()),
            ..Default::default()
        };
        let planned = plan(&raw, &source(), &options(), ContentKind::None);
        assert_eq!(planned.body, "Just a summary");
        assert_eq!(planned.front.content, ContentKind::None);
        assert!(planned.html.is_none());
        assert_eq!(planned.dir, "items/blog/2026/09");

        let no_html = Source {
            html: false,
            ..source()
        };
        let with_content = RawItem {
            content_html: Some("<p>x</p>".into()),
            ..raw
        };
        assert!(
            plan(&with_content, &no_html, &options(), ContentKind::Feed)
                .html
                .is_none()
        );
    }

    #[test]
    fn first_import_keeps_only_the_newest_bounded_entries() {
        let mut items = (1..=5)
            .map(|day| RawItem {
                title: day.to_string(),
                published: Some(Utc.with_ymd_and_hms(2026, 9, day, 0, 0, 0).unwrap()),
                ..Default::default()
            })
            .collect();
        keep_newest(&mut items, 2);
        assert_eq!(
            items
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["5", "4"]
        );
    }

    #[test]
    fn aggr_imports_are_not_capped_by_the_feed_safety_limit() {
        let items = (1..=5)
            .map(|day| RawItem {
                title: day.to_string(),
                published: Some(Utc.with_ymd_and_hms(2026, 9, day, 0, 0, 0).unwrap()),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        let mut feed_items = items.clone();
        apply_first_import_limit(
            &mut feed_items,
            &Engine::Feed {
                url: Url::parse("https://example.com/feed.xml").unwrap(),
            },
            2,
        );
        assert_eq!(feed_items.len(), 2);

        let mut aggr_items = items;
        apply_first_import_limit(
            &mut aggr_items,
            &Engine::Aggr {
                url: Url::parse("https://git.example/friend/reads.git").unwrap(),
                branch: "aggr".into(),
                sources: vec![],
                limit: None,
            },
            2,
        );
        assert_eq!(aggr_items.len(), 5);
    }

    #[test]
    fn state_is_persisted_for_visible_changes_or_the_dev_cache() {
        use StatePolicy::{DevCache, PersistentBranch};

        for (added, repaired, metadata_changed, policy, expected) in [
            (0, 0, false, PersistentBranch, false),
            (1, 0, false, PersistentBranch, true),
            (0, 1, false, PersistentBranch, true),
            (0, 0, true, PersistentBranch, true),
            (0, 0, false, DevCache, true),
            (2, 1, true, DevCache, true),
        ] {
            assert_eq!(
                should_persist_state(added, repaired, metadata_changed, policy),
                expected,
                "added={added} repaired={repaired} metadata_changed={metadata_changed} dev={}",
                policy == DevCache
            );
        }
    }

    #[test]
    fn upstream_metadata_changes_ignore_title_whitespace_but_not_renames_or_site_urls() {
        let previous = SourceState {
            title: Some("Demo blog".into()),
            site_url: Some("https://demo.example/".into()),
            ..Default::default()
        };
        let reformatted = SourceState {
            title: Some("  Demo \n\t blog  ".into()),
            ..previous.clone()
        };
        assert!(!source_metadata_changed(&previous, &reformatted));
        assert!(!source_metadata_changed(&previous, &previous));
        let renamed = SourceState {
            title: Some("Demo blog renamed".into()),
            ..previous.clone()
        };
        assert!(source_metadata_changed(&previous, &renamed));
        let moved = SourceState {
            site_url: Some("https://demo.example/blog/".into()),
            ..previous.clone()
        };
        assert!(source_metadata_changed(&previous, &moved));
        // The first title an upstream reports travels with the source's first item, so a
        // duplicate-only source leaves no trace; a blank title is no title at all.
        let discovered = SourceState {
            title: Some("Demo blog".into()),
            site_url: Some("https://demo.example/".into()),
            ..Default::default()
        };
        assert!(!source_metadata_changed(
            &SourceState::default(),
            &discovered
        ));
        let blank = SourceState {
            title: Some("   ".into()),
            ..previous.clone()
        };
        assert!(source_metadata_changed(&previous, &blank));
        let never_titled = SourceState {
            title: Some("   ".into()),
            ..Default::default()
        };
        assert!(!source_metadata_changed(
            &never_titled,
            &SourceState::default()
        ));
    }

    #[test]
    fn apply_validators_keeps_the_public_endpoint_only_when_the_source_persists_it() {
        let validators = || sources::Validators {
            etag: Some("\"v1\"".into()),
            last_modified: Some("Tue, 01 Sep 2026 10:00:00 GMT".into()),
            body_hash: Some("abc".into()),
            resolved_url: Some(
                "https://user:t0ken@blog.example/rss?page=2&api_key=secret#latest".into(),
            ),
        };
        let mut persisted = SourceState::default();
        apply_validators(validators(), &mut persisted, &source());
        assert_eq!(
            persisted.resolved_url.as_deref(),
            Some("https://blog.example/rss?page=2"),
            "credentials, sensitive query keys, and fragments never reach state.toml"
        );
        assert_eq!(persisted.etag.as_deref(), Some("\"v1\""));
        assert_eq!(
            persisted.last_modified.as_deref(),
            Some("Tue, 01 Sep 2026 10:00:00 GMT")
        );
        assert_eq!(persisted.body_hash.as_deref(), Some("abc"));

        let mut ephemeral = SourceState {
            resolved_url: Some("https://stale.example/feed".into()),
            ..Default::default()
        };
        let source = Source {
            persist_endpoint: false,
            ..source()
        };
        apply_validators(validators(), &mut ephemeral, &source);
        assert_eq!(
            ephemeral.resolved_url, None,
            "a source that does not persist its endpoint also forgets a stale one"
        );
        assert_eq!(ephemeral.body_hash.as_deref(), Some("abc"));

        let mut unresolved = SourceState::default();
        apply_validators(
            sources::Validators {
                resolved_url: Some("not a url".into()),
                ..Default::default()
            },
            &mut unresolved,
            &self::source(),
        );
        assert_eq!(unresolved.resolved_url, None);
    }

    #[test]
    fn source_errors_never_leak_the_expanded_url_or_its_credentials() {
        let mut configured = source();
        configured.engine = Engine::Feed {
            url: Url::parse("https://user:t0ken@blog.example/feed").unwrap(),
        };
        let sanitized =
            |error: anyhow::Error| format!("{:#}", sanitized_source_error(&configured, &error));

        assert_eq!(
            sanitized(anyhow::anyhow!(
                "fetching https://user:t0ken@blog.example/feed: 401"
            )),
            "fetching https://blog.example/feed: 401"
        );
        // A redirect target carries the same credentials in a URL that no longer matches.
        assert_eq!(
            sanitized(anyhow::anyhow!(
                "redirected to https://user:t0ken@blog.example/rss.xml: 401"
            )),
            "redirected to https://[redacted]:[redacted]@blog.example/rss.xml: 401"
        );
        assert_eq!(
            sanitized(anyhow::anyhow!("bearer t0ken rejected")),
            "bearer [redacted] rejected"
        );
        // Chained contexts are flattened first, and every occurrence goes.
        assert_eq!(
            sanitized(
                anyhow::anyhow!("401 for https://user:t0ken@blog.example/feed")
                    .context("fetching https://user:t0ken@blog.example/feed")
            ),
            "fetching https://blog.example/feed: 401 for https://blog.example/feed"
        );

        let mut anonymous = source();
        anonymous.public_url = None;
        anonymous.engine = Engine::Feed {
            url: Url::parse("https://user:t0ken@blog.example/feed").unwrap(),
        };
        assert_eq!(
            format!(
                "{:#}",
                sanitized_source_error(
                    &anonymous,
                    &anyhow::anyhow!("GET https://user:t0ken@blog.example/feed failed")
                )
            ),
            "GET [source URL] failed"
        );

        // Without credentials only the exact expanded URL is rewritten.
        let plain = source();
        assert_eq!(
            format!(
                "{:#}",
                sanitized_source_error(
                    &plain,
                    &anyhow::anyhow!("GET https://blog.example/other: 500 (user agent)")
                )
            ),
            "GET https://blog.example/other: 500 (user agent)"
        );
    }

    #[test]
    fn from_existing_keeps_the_stored_front_and_body_and_takes_the_path() {
        let item = Item {
            path: "items/blog/2026/09/2026-09-01-post".into(),
            front: FrontMatter {
                source: "blog".into(),
                title: "Post".into(),
                ..Default::default()
            },
            body: "Stored body.\n".into(),
        };
        let planned = Planned::from_existing(item.clone(), &item.path).unwrap();
        assert_eq!(planned.dir, "items/blog/2026/09");
        assert_eq!(planned.stem, "2026-09-01-post");
        assert_eq!(planned.front, item.front);
        assert_eq!(planned.body, item.body);
        assert!(planned.html.is_none());
        assert!(Planned::from_existing(item, "items/other/2026/09/post").is_err());
    }

    #[test]
    fn existing_item_paths_must_name_a_file_inside_the_source_directory() {
        let planned = || Planned {
            dir: String::new(),
            stem: String::new(),
            front: FrontMatter {
                source: "blog".into(),
                ..Default::default()
            },
            body: String::new(),
            html: None,
        };
        let mut accepted = planned();
        use_existing_path(&mut accepted, "items/blog/2026/09/2026-09-01-post").unwrap();
        assert_eq!(accepted.dir, "items/blog/2026/09");
        assert_eq!(accepted.stem, "2026-09-01-post");
        let mut flat = planned();
        use_existing_path(&mut flat, "items/blog/post").unwrap();
        assert_eq!(
            (flat.dir.as_str(), flat.stem.as_str()),
            ("items/blog", "post")
        );

        for rejected in [
            "../items/blog/2026/09/post",
            "/items/blog/2026/09/post",
            "items/blog/../other/2026/09/post",
            "items/other/2026/09/post",
            "items/blogs/2026/09/post",
            "posts/blog/2026/09/post",
            "items/blog",
            "items",
            "",
        ] {
            let mut item = planned();
            let error = use_existing_path(&mut item, rejected).unwrap_err();
            assert!(
                error.to_string().contains("invalid existing item path"),
                "{rejected:?}: {error}"
            );
            assert!(
                item.dir.is_empty() && item.stem.is_empty(),
                "{rejected:?} must not assign a path"
            );
        }
    }
}
