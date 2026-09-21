//! Static site generation: the data tree + config + git facts in, a directory of files out.
//! The planning half (`plan`) is pure; `build` does the IO.

mod assets;
mod budget;
mod compressed_media;
pub mod context;
mod directory;
mod display;
mod document;
pub(crate) mod interactive;
pub(crate) mod item_type;
mod native_media;
mod output_dir;
pub mod outputs;
mod page;
mod pagefind;
mod parallel;
mod related;
pub mod render;
mod source_index;
pub(crate) mod video;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sha1::{Digest as _, Sha1};

use crate::config::{Config, SiteConfig, SiteIdentityKind, Source};
use crate::content;
use crate::model::Item;
use crate::store::Store;
use context::{
    ArticleLinkCtx, ArticlePreviewCtx, BuildCtx, GitHubLinks, ItemCtx, ItemOptions, PaginatorCtx,
    SiteCtx, SiteIdentityCtx, SourceCtx,
};
use directory::{
    Taxonomy, TaxonomyIndex, feed_items, indexed_items, source_contexts, source_members,
    taxonomy_index, write_collection_feeds,
};
use output_dir::MARKER;
pub(crate) use output_dir::prepare_out_dir;
use page::{ListPage, Pages, SharedCtx, SimplePage, archive_modified_at, default_site_description};
use render::{Layers, Renderer};

const EXCERPT_CHARS: usize = 240;
const RECOMMENDATION_CARD_COUNT: usize = 1;

fn is_visible_item(item: &Item) -> bool {
    !item.front.hidden
        && !url::Url::parse(&item.front.link)
            .is_ok_and(|url| crate::sources::youtube::is_short_url(&url))
}

/// Pick one archive page per article without changing retained data or breaking old page URLs.
fn visible_archive(items: Vec<Item>, sources: &[Source]) -> (Vec<Item>, Vec<(String, String)>) {
    let publisher_hosts: BTreeMap<_, _> = sources
        .iter()
        .filter_map(|source| {
            source.public_url.as_deref().and_then(|url| {
                let host = context::domain_of(url);
                (!host.is_empty()).then_some((source.slug.as_str(), host))
            })
        })
        .collect();
    let is_publisher = |item: &Item| {
        publisher_hosts
            .get(item.front.source.as_str())
            .is_some_and(|host| *host == context::domain_of(&item.front.link))
    };
    let mut groups = BTreeMap::<String, Vec<Item>>::new();
    let mut visible = Vec::new();
    for item in items {
        if !is_visible_item(&item) {
            continue;
        }
        let key = crate::model::normalize_link(&item.front.link);
        if key.is_empty() {
            visible.push(item);
        } else {
            groups.entry(key).or_default().push(item);
        }
    }
    let mut redirects = Vec::new();
    for mut group in groups.into_values() {
        group.sort_by(|a, b| {
            is_publisher(b)
                .cmp(&is_publisher(a))
                .then_with(|| b.body.len().cmp(&a.body.len()))
                .then_with(|| a.path.cmp(&b.path))
        });
        let mut group = group.into_iter();
        let winner = group.next().expect("nonempty article group");
        let target = context::item_url(&winner.path);
        for duplicate in group {
            let previous = context::item_url(&duplicate.path);
            if previous != target {
                redirects.push((previous, target.clone()));
            }
        }
        visible.push(winner);
    }
    (visible, redirects)
}

/// Facts about the build that do not come from the data tree.
pub struct BuildInfo {
    pub out: PathBuf,
    pub base_url: Option<String>,
    pub config_sha: Option<String>,
    /// Repository-relative path of the root config file.
    pub config_path: Option<String>,
    pub data_sha: Option<String>,
    pub generation: String,
    pub now: DateTime<Utc>,
    /// Production build: absolute URLs from `base_url`, CNAME for custom domains.
    pub release: bool,
    /// Local dev snapshots remain unindexable even when emulating a release build.
    pub development: bool,
    /// Include the persistent publication cache marker in the measured output budget.
    pub render_cache_key: Option<String>,
    pub discussions: crate::discussions::ResolutionSet,
    /// Search-index cache shared across template-only rebuilds.
    pub pagefind_cache: Option<PathBuf>,
}

/// Which items belong on the recent home feed; archives render every retained item.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Window {
    pub rendered: Vec<usize>,
}

/// Split items (sorted newest first, hidden already removed) into the recent feed and its tail.
pub fn window(
    dates: &[DateTime<Utc>],
    now: DateTime<Utc>,
    max_items: usize,
    max_age_days: u32,
) -> Window {
    let cutoff = now - Duration::days(i64::from(max_age_days));
    let mut out = Window::default();
    for (index, date) in dates.iter().enumerate() {
        if out.rendered.len() < max_items && *date >= cutoff {
            out.rendered.push(index);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pager {
    pub path: String,
    pub range: std::ops::Range<usize>,
    pub context: PaginatorCtx,
}

fn navigation_path(path: &str) -> String {
    if path.is_empty() {
        "./".to_string()
    } else {
        path.to_string()
    }
}

/// Zola-style pagers for `total` entries: the first lives at `prefix`, then `page/N/`.
pub fn paginate(prefix: &str, total: usize, per_page: usize) -> Vec<Pager> {
    assert!(per_page > 0, "paginate_by must be positive");
    let number_pagers = total.div_ceil(per_page).max(1);
    let paths: Vec<_> = (1..=number_pagers)
        .map(|current_index| {
            if current_index == 1 {
                prefix.to_string()
            } else {
                format!("{prefix}page/{current_index}/")
            }
        })
        .collect();
    let first = navigation_path(&paths[0]);
    let last = navigation_path(paths.last().expect("at least one pager"));
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let current_index = index + 1;
            let start = index * per_page;
            Pager {
                path: path.clone(),
                range: start..(start + per_page).min(total),
                context: PaginatorCtx {
                    paginate_by: per_page,
                    base_url: format!("{prefix}page/"),
                    number_pagers,
                    first: first.clone(),
                    last: last.clone(),
                    previous: index
                        .checked_sub(1)
                        .map(|previous| navigation_path(&paths[previous])),
                    next: paths.get(index + 1).map(|next| navigation_path(next)),
                    current_index,
                    total_items: total,
                    offset: start,
                },
            }
        })
        .collect()
}

/// Names the precache from durable build inputs. A no-op rebuild therefore reuses its cache,
/// while a changed worker refreshes the same keys during installation.
pub fn cache_version(build: &BuildCtx) -> String {
    fn short(sha: Option<&str>) -> &str {
        sha.map_or("local", |sha| &sha[..sha.len().min(12)])
    }
    format!(
        "{}-{}-{}-{}",
        build.version,
        short(build.data_sha.as_deref()),
        short(build.config_sha.as_deref()),
        short(Some(&build.generation))
    )
}

/// Reader releases depend on shipped code, never on values substituted while rendering it.
fn app_version(binary_version: &str, layers: &Layers) -> Result<String> {
    fn field(hash: &mut Sha1, bytes: &[u8]) {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    let mut hash = Sha1::new();
    field(&mut hash, b"aggr-app-v1");
    field(&mut hash, binary_version.as_bytes());
    for (kind, names) in [
        ("templates", layers.template_names()?),
        ("static", layers.static_names()?),
    ] {
        for name in names {
            let bytes = layers
                .read(kind, &name)?
                .with_context(|| format!("{kind}/{name} vanished during build"))?;
            field(&mut hash, format!("{kind}/{name}").as_bytes());
            field(&mut hash, &bytes);
        }
    }
    Ok(hex::encode(hash.finalize()))
}

/// Fingerprint of data that can affect rendered output, including age bands and the home-feed
/// cutoff. It changes only at a semantic boundary, so repeated builds stay instant while an
/// unchanged repository can never keep a stale 24-hour marker or expired river entry.
pub fn render_generation(items: &[Item], site: &SiteConfig, now: DateTime<Utc>) -> String {
    fn field(hash: &mut Sha1, bytes: &[u8]) {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    let cutoff = now - Duration::days(i64::from(site.max_age_days));
    let mut ordered: Vec<_> = items.iter().filter(|item| !item.front.hidden).collect();
    ordered.sort_by(|a, b| a.path.cmp(&b.path));
    let mut hash = Sha1::new();
    for item in ordered {
        field(&mut hash, item.path.as_bytes());
        field(
            &mut hash,
            &serde_json::to_vec(&item.front).unwrap_or_default(),
        );
        field(&mut hash, item.body.as_bytes());
        field(
            &mut hash,
            context::age_band(now, item.created_at()).as_bytes(),
        );
        field(&mut hash, &[u8::from(item.created_at() >= cutoff)]);
        field(
            &mut hash,
            &[u8::from(budget::full_quality(
                item.created_at(),
                now,
                site.media_full_quality_days,
            ))],
        );
    }
    hex::encode(hash.finalize())
}

/// `https://user.github.io/repo/` → `/repo/`; anything unparsable → `/`.
pub fn base_path(base_url: Option<&str>) -> String {
    let path = base_url
        .and_then(|url| url::Url::parse(url).ok())
        .map(|url| url.path().to_string())
        .unwrap_or_default();
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        "/".into()
    } else {
        format!("/{trimmed}/")
    }
}

/// Relative reference from a generated page back to the output root. Directory routes end in
/// `/`; standalone root files such as `offline.html` do not add a level.
pub fn relative_root(path: &str) -> String {
    let depth = path
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .count()
        .saturating_sub(usize::from(!path.is_empty() && !path.ends_with('/')));
    if depth == 0 {
        "./".into()
    } else {
        "../".repeat(depth)
    }
}

/// What `sw.js` sees: the cache name and the revisioned install-time fetch list.
#[derive(Serialize)]
struct SwCtx<'a> {
    site: &'a SiteCtx,
    build: &'a BuildCtx,
    version: String,
    precache: Vec<assets::PrecacheEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub pages: usize,
    pub items: usize,
    pub stubs: usize,
}

pub fn build(
    config: &Config,
    sources: &[Source],
    store: &Store,
    project_root: &Path,
    info: &BuildInfo,
) -> Result<Summary> {
    let limit = config.site.build_max_bytes;
    let mut attempt = budget::Attempt::new(limit);
    loop {
        let mut media_budget = budget::MediaBudget::new(attempt.allowance());
        let summary = build_once(
            config,
            sources,
            store,
            project_root,
            info,
            &mut media_budget,
        )?;
        if let Some(key) = &info.render_cache_key {
            write(
                &info.out.join(crate::cache::RENDER_KEY_FILE),
                key.as_bytes(),
            )?;
        }
        let bytes = budget::output_bytes(&info.out)?;
        if bytes <= limit {
            log::info!(
                "site size: {bytes} / {limit} bytes; {} media groups left at their publisher to fit",
                media_budget.omitted
            );
            return Ok(summary);
        }
        let Some(next) = attempt.next(bytes - limit, media_budget.used) else {
            bail!(
                "site text and required assets need {bytes} bytes, exceeding [site] build_max_bytes = {limit}; increase the build limit or use a host with more capacity; no archived articles were removed"
            );
        };
        log::info!(
            "site is {bytes} bytes; reserving space for all article text and rebuilding with {} media bytes",
            next.allowance()
        );
        attempt = next;
    }
}

/// Cache hits and final restoration must obey the same limit as a fresh render.
pub(crate) fn verify_output_budget(out: &Path, limit: u64) -> Result<()> {
    let bytes = budget::output_bytes(out)?;
    if bytes > limit {
        bail!(
            "generated site is {bytes} bytes, exceeding [site] build_max_bytes = {limit}; remove modified output or rebuild with a larger budget"
        );
    }
    Ok(())
}

fn build_once(
    config: &Config,
    sources: &[Source],
    store: &Store,
    project_root: &Path,
    info: &BuildInfo,
    media_budget: &mut budget::MediaBudget,
) -> Result<Summary> {
    let started = std::time::Instant::now();
    let mut phases: Vec<(&str, std::time::Duration)> = Vec::new();
    let mut phase_started = started;
    let mut phase = |name: &'static str| {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(phase_started);
        log::debug!("build {name}: {:.3}s", elapsed.as_secs_f64());
        phases.push((name, elapsed));
        phase_started = now;
    };
    let out = &info.out;
    prepare_out_dir(out)?;
    let config_name = info
        .config_path
        .as_deref()
        .and_then(|path| Path::new(path).file_name())
        .unwrap_or_else(|| std::ffi::OsStr::new("aggr.toml"));
    let config_source = project_root.join(config_name);
    if config_source.is_file() {
        write(&out.join("aggr.toml"), &std::fs::read(&config_source)?)?;
    }

    let base = base_path(info.base_url.as_deref());
    let repository = config.repository();
    let config_path = info.config_path.as_deref().unwrap_or("aggr.toml");
    let description = config
        .site
        .description
        .clone()
        .unwrap_or_else(|| default_site_description(&config.site.title));
    let discussion_shortcuts = context::discussion_shortcuts(&config.networks);
    let mut site = SiteCtx {
        preferences: config.site.preferences.browser_defaults()?,
        preference_schema: config.site.preferences.schema(),
        title: config.site.title.clone(),
        description,
        identity: config
            .site
            .identity
            .as_ref()
            .map(|identity| SiteIdentityCtx {
                kind: match identity.kind {
                    SiteIdentityKind::Person => "Person",
                    SiteIdentityKind::Organization => "Organization",
                },
                name: identity.name.clone(),
                url: identity.url.as_ref().map(ToString::to_string),
                same_as: identity.same_as.iter().map(ToString::to_string).collect(),
            }),
        language: config.site.language.clone(),
        og_locale: context::og_locale(&config.site.language),
        base_path: base.clone(),
        base_url: info.base_url.clone().map(|url| ensure_trailing_slash(&url)),
        indexing: config.site.indexing && info.release && !info.development,
        repository: repository.clone(),
        data_branch: config.store.branch.clone(),
        network_url: outputs::AGGR_NETWORK,
        instance_type_url: outputs::AGGR_INSTANCE_TYPE,
        pwa: config.site.pwa,
        config_page_url: repository.as_deref().and_then(|repository| {
            info.config_sha
                .as_deref()
                .map(|sha| format!("https://github.com/{repository}/blob/{sha}/{config_path}"))
        }),
        config_url: repository.as_deref().and_then(|repository| {
            info.config_sha.as_deref().map(|sha| {
                format!(
                    "https://raw.githubusercontent.com/{repository}/{sha}/{}",
                    config_path
                )
            })
        }),
        has_categories: false,
        discussions: config
            .networks
            .iter()
            .zip(discussion_shortcuts)
            .map(|(d, shortcut)| context::DiscussionLinkCtx {
                name: context::compact_name(&d.name),
                url: d.url.clone(),
                shortcut,
                found: false,
                score: None,
            })
            .collect(),
        entry_shortcuts: Vec::new(),
        params: config.site.params.clone(),
    };
    let layers = theme_layers(config, project_root)?;
    let mut build_ctx = BuildCtx {
        time: info.now,
        version: env!("CARGO_PKG_VERSION").to_string(),
        app_version: app_version(env!("CARGO_PKG_VERSION"), &layers)?,
        content_version: String::new(),
        config_sha: info.config_sha.clone(),
        data_sha: info.data_sha.clone(),
        generation: info.generation.clone(),
        release: info.release,
    };
    let links = repository.as_deref().map(|repository| GitHubLinks {
        repository,
        branch: &config.store.branch,
        data_sha: info.data_sha.as_deref(),
    });

    let renderer = Renderer::new(layers)?;

    // Sources: config order, enriched with stored state and counts.
    let status = store.status()?;
    let stored_items: Vec<_> = store.items()?.into_iter().filter(is_visible_item).collect();
    let mut source_ctxs = source_contexts(sources, store, &status, &stored_items)?;
    let stored_sources = source_index::capture_sources(&stored_items);
    let (mut all_items, duplicate_redirects) = visible_archive(stored_items, sources);
    for item in &mut all_items {
        item.body = content::normalize_subscription_metadata(&item.body, &mut item.front);
        item.body = content::normalize_aggregator_metadata(&item.body, &mut item.front);
        let (body, boundary_labels) = content::normalize_article_body(
            &item.body,
            &item.front.title,
            item.front.published,
            &item.front.source,
        );
        item.body = body;
        item.front.labels =
            crate::model::normalize_labels(item.front.labels.iter().chain(&boundary_labels));
        item.body = crate::threads::clean_archived_thread(&item.body, &item.front.link);
    }
    all_items.sort_by(|a, b| {
        b.created_at()
            .cmp(&a.created_at())
            .then_with(|| b.path.cmp(&a.path))
    });
    let source_by_slug: BTreeMap<&str, &SourceCtx> =
        source_ctxs.iter().map(|s| (s.slug.as_str(), s)).collect();

    let dates: Vec<_> = all_items.iter().map(Item::created_at).collect();
    let window = window(
        &dates,
        info.now,
        config.site.max_items,
        config.site.max_age_days,
    );
    phase("archive preparation");

    // The bounded window controls only the river. Source/category/tag pages, search, and clean
    // article pages are archives over the retained database; `[store]` retention is the explicit
    // knob for bounding those. This keeps old sources browsable without making the home feed stale.
    let prepared_bodies = parallel::map(&all_items, |item| {
        Ok(content::PreparedMarkdown::new(&item.body))
    })?;
    let prepared_by_path: BTreeMap<_, _> = all_items
        .iter()
        .zip(&prepared_bodies)
        .map(|(item, prepared)| (item.path.as_str(), prepared))
        .collect();
    let mut archive_items = Vec::with_capacity(all_items.len());
    let mut article_images = BTreeMap::<String, Vec<content::LocalImage>>::new();
    let mut written_assets = assets::Published::new();
    let media_memo = assets::MediaMemo::new(store.image_cache().cloned());
    let compact_cache = info
        .pagefind_cache
        .as_deref()
        .map(|root| crate::cache::Namespace::DeploymentMedia.dir(root));
    let media_enabled = media_budget.allowance() > 0;
    if !media_enabled {
        media_budget.omitted = all_items
            .iter()
            .map(|item| {
                item.front.images.len()
                    + usize::from(item.front.preview.is_some())
                    + usize::from(item.front.document.is_some())
            })
            .sum();
    }
    let derive_preview = |item: &Item| {
        sources
            .iter()
            .find(|source| source.slug == item.front.source)
            .map_or(config.fetch.previews, |source| source.previews)
    };
    // Reading, validating and decoding archived media dominates this phase, so it runs on worker
    // threads one window at a time, which bounds the assets held in memory. Publishing stays on
    // this thread in item order: the dedupe map, memo state and archive order never depend on
    // timing, so every worker count produces the same bytes.
    let media_window = parallel::window();
    for (items, prepared) in all_items
        .chunks(media_window)
        .zip(prepared_bodies.chunks(media_window))
    {
        let media = parallel::map(items, |item| {
            if !media_enabled {
                return Ok(assets::ItemMedia::default());
            }
            assets::ItemMedia::gather(
                store,
                item,
                derive_preview(item),
                &media_memo,
                !budget::full_quality(
                    item.created_at(),
                    info.now,
                    config.site.media_full_quality_days,
                ),
                compact_cache.as_deref(),
            )
        })?;
        for ((item, prepared), media) in items.iter().zip(prepared).zip(media) {
            let (source_name, category) = source_by_slug
                .get(item.front.source.as_str())
                .map(|s| (s.name.as_str(), s.category.as_deref()))
                .unwrap_or((item.front.source.as_str(), None));
            let excerpt = item
                .front
                .summary
                .clone()
                .filter(|_| prepared.resources().is_empty())
                .map(|s| content::excerpt(&s, EXCERPT_CHARS))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| prepared.excerpt(EXCERPT_CHARS));
            let mut ctx = ItemCtx::from_item(
                item,
                ItemOptions {
                    reading_metrics: prepared.reading_metrics(),
                    source_name,
                    category,
                    links: links.as_ref(),
                    excerpt,
                    discussions: &config.networks,
                    resolutions: &info.discussions,
                    now: info.now,
                },
            );
            if let Some(source) = source_by_slug.get(item.front.source.as_str()) {
                ctx.set_source(source);
            }
            ctx.resources = prepared.resources().to_vec();
            let (preview, local_images, local_document) =
                media.publish(out, &mut written_assets, media_budget)?;
            ctx.preview = preview;
            ctx.document = document::DocumentCtx::from_item(item).map(|mut document| {
                document.local_url = local_document.map(|mut local| {
                    if let Some(fragment) = url::Url::parse(&document.url)
                        .ok()
                        .and_then(|url| url.fragment().map(str::to_owned))
                    {
                        local.push('#');
                        local.push_str(&fragment);
                    }
                    local
                });
                document
            });
            if !local_images.is_empty() {
                article_images.insert(item.path.clone(), local_images);
            }
            archive_items.push(ctx);
        }
    }
    phase("media and item metadata");

    let subscriptions = source_ctxs.clone();
    source_index::resolve(
        &mut archive_items,
        &mut source_ctxs,
        &stored_sources,
        &duplicate_redirects,
    );

    let recommendation_text: Vec<_> = prepared_bodies
        .iter()
        .map(content::PreparedMarkdown::plain_text)
        .collect();
    let recommendations = related::resolve(&archive_items, &recommendation_text);
    let article_links: Vec<_> = archive_items.iter().map(ArticleLinkCtx::from).collect();
    for (item, recommendation) in archive_items.iter_mut().zip(recommendations) {
        let previous = recommendation.previous;
        let next = recommendation.next;
        item.previous_article = previous.map(|index| article_links[index].clone());
        item.next_article = next.map(|index| article_links[index].clone());
        item.recommended_articles = recommendation
            .articles
            .into_iter()
            .filter(|index| Some(*index) != previous && Some(*index) != next)
            .take(RECOMMENDATION_CARD_COUNT)
            .map(|index| article_links[index].clone())
            .collect();
    }

    let river_items: Vec<ItemCtx> = window
        .rendered
        .iter()
        .map(|&index| archive_items[index].clone())
        .collect();
    site.entry_shortcuts = river_items
        .iter()
        .take(9)
        .map(|item| item.url.clone())
        .collect();
    let source_members = source_members(&archive_items);
    let TaxonomyIndex {
        terms: categories,
        members: category_members,
    } = taxonomy_index(&archive_items, Taxonomy::Categories);
    site.has_categories = !categories.is_empty();
    let TaxonomyIndex {
        terms: tags,
        members: tag_members,
    } = taxonomy_index(&archive_items, Taxonomy::Tags);
    let mut pages = 0;
    let per_page = config.site.items_per_page;
    build_ctx.content_version = crate::model::sha1_hex(serde_json::to_vec(&(
        &build_ctx.generation,
        &build_ctx.data_sha,
        &build_ctx.config_sha,
        &site,
        &source_ctxs,
        &archive_items,
        &written_assets,
        per_page,
        config.site.max_items,
        config.site.max_age_days,
        config.site.build_max_bytes,
        config.site.media_full_quality_days,
    ))?);
    write(
        &out.join("updates.json"),
        outputs::updates(&site, &build_ctx)?.as_bytes(),
    )?;
    let pages_ctx = Pages::new(
        &site,
        &renderer,
        SharedCtx::new(&site, &build_ctx, &source_ctxs, &categories, &tags),
        &archive_items,
        per_page,
    );
    let mut sitemap_urls: Vec<outputs::SitemapUrl> = Vec::new();
    pages += pages_ctx.write_list(
        out,
        ListPage {
            kind: "river",
            title: &config.site.title,
            prefix: "",
            list: &river_items,
            source: None,
            category: None,
        },
        &mut sitemap_urls,
    )?;
    for source in &source_ctxs {
        let list = indexed_items(
            &archive_items,
            source_members.get(&source.slug).map(Vec::as_slice),
        );
        pages += pages_ctx.write_list(
            out,
            ListPage {
                kind: "source",
                title: &source.name,
                prefix: &source.page,
                list: &list,
                source: Some(source),
                category: None,
            },
            &mut sitemap_urls,
        )?;
        let feed_items = feed_items(&list, &prepared_by_path, per_page);
        write_collection_feeds(
            out,
            &site,
            &build_ctx,
            &source.name,
            &source.page,
            &feed_items,
        )?;
    }
    for category in &categories {
        let list = indexed_items(
            &archive_items,
            category_members.get(&category.slug).map(Vec::as_slice),
        );
        pages += pages_ctx.write_list(
            out,
            ListPage {
                kind: "category",
                title: &category.name,
                prefix: &category.page,
                list: &list,
                source: None,
                category: Some(category),
            },
            &mut sitemap_urls,
        )?;
        let feed_items = feed_items(&list, &prepared_by_path, per_page);
        write_collection_feeds(
            out,
            &site,
            &build_ctx,
            &category.name,
            &category.page,
            &feed_items,
        )?;
    }
    for tag in &tags {
        let list = indexed_items(
            &archive_items,
            tag_members.get(&tag.slug).map(Vec::as_slice),
        );
        pages += pages_ctx.write_list(
            out,
            ListPage {
                kind: "tag",
                title: &tag.name,
                prefix: &tag.page,
                list: &list,
                source: None,
                category: Some(tag),
            },
            &mut sitemap_urls,
        )?;
        let feed_items = feed_items(&list, &prepared_by_path, per_page);
        write_collection_feeds(out, &site, &build_ctx, &tag.name, &tag.page, &feed_items)?;
    }

    let archive_updated = archive_items.iter().map(archive_modified_at).max();
    for path in ["browse/", "sources/", "categories/", "tags/"] {
        if let Some(url) = site.absolute(path) {
            sitemap_urls.push(outputs::SitemapUrl::new(url, archive_updated));
        }
    }

    write(
        &out.join("browse/index.html"),
        pages_ctx
            .simple(SimplePage::new(
                "browse",
                "Browse",
                "browse/",
                "browse.html",
            ))?
            .as_bytes(),
    )?;
    for (kind, title) in [
        ("categories", "Categories"),
        ("sources", "Sources"),
        ("tags", "Tags"),
    ] {
        write(
            &out.join(kind).join("index.html"),
            pages_ctx
                .simple(SimplePage::new(
                    kind,
                    title,
                    &format!("{kind}/"),
                    "browse.html",
                ))?
                .as_bytes(),
        )?;
    }
    write(
        &out.join("preferences/index.html"),
        pages_ctx
            .simple(SimplePage::new(
                "preferences",
                "Preferences",
                "preferences/",
                "preferences.html",
            ))?
            .as_bytes(),
    )?;
    write(
        &out.join("404.html"),
        pages_ctx
            .simple(SimplePage::new("404", "Not found", "404.html", "404.html"))?
            .as_bytes(),
    )?;
    pages += 6;
    phase("feed and directory pages");

    let article_pairs: Vec<_> = archive_items.iter().zip(&all_items).collect();
    let article_sitemaps = parallel::map(&article_pairs, |&(ctx, item)| {
        let mut ctx = ctx.clone();
        ctx.video = video::VideoCtx::from_url(&item.front.link);
        ctx.interactive = interactive::InteractiveCtx::from_item(item);

        ctx.native_media = native_media::NativeMediaCtx::from_urls(
            &item.front.link,
            item.front
                .extra
                .get("audio_url")
                .and_then(|value| value.as_str()),
        );
        if ctx.is_youtube
            && let Ok(url) = url::Url::parse(&item.front.link)
            && let Some(image) = article_images.get(&item.path).and_then(|images| {
                let candidates = crate::sources::youtube::poster_candidates(&url);
                images.iter().find(|image| {
                    candidates
                        .iter()
                        .any(|candidate| candidate.url.as_str() == image.source)
                })
            })
        {
            ctx.article_preview = Some(ArticlePreviewCtx::from_image(image));
        }
        let prepared_markdown = prepared_by_path[ctx.path.as_str()];
        let portable_html = prepared_markdown.portable_html();
        if ctx.video.is_none()
            && ctx.document.is_none()
            && ctx.interactive.is_none()
            && let Ok(base) = url::Url::parse(&item.front.link)
        {
            ctx.article_preview = ArticlePreviewCtx::lead_image(
                portable_html,
                &base,
                article_images
                    .get(&item.path)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            );
        }
        let dimensions = if portable_html.contains("<img ") {
            let retained_html = store.read_html(item)?;
            let base = url::Url::parse(&item.front.link).ok();
            retained_html
                .as_deref()
                .map(|html| content::image_dimensions(html, base.as_ref()))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let article_url = url::Url::parse(&item.front.link).ok();
        let local_images = article_images
            .get(&item.path)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let (body_html, has_margin_notes) =
            content::margin_notes(&content::external_body_links(&content::embed_body_videos(
                &content::anchor_headings(
                    &prepared_markdown.reader_html_with_images(local_images, &dimensions),
                    article_url.as_ref(),
                ),
                local_images,
            )));
        ctx.body_html = Some(body_html);
        ctx.has_margin_notes = has_margin_notes;
        let dir = out.join(&ctx.url);
        let representation = out.join(ctx.url.trim_end_matches('/'));
        write(
            &dir.join("index.html"),
            pages_ctx
                .simple(SimplePage {
                    item: Some(&ctx),
                    ..SimplePage::new("item", &ctx.title, &ctx.url, "item.html")
                })?
                .as_bytes(),
        )?;
        // Alternate representations remain portable across hosts and mirrors. Only the rendered
        // archive page substitutes immutable site-local companions for publisher image URLs.
        let published_body = outputs::published_markdown_body(&ctx, &item.body);
        ctx.body_html = Some(if published_body == item.body {
            portable_html.to_string()
        } else {
            content::PreparedMarkdown::new(&published_body)
                .portable_html()
                .to_string()
        });
        let mut published_front = item.front.clone();
        published_front.title = ctx.title.clone();
        let markdown = crate::store::frontmatter::render(&published_front, &published_body)?;
        write(&representation.with_extension("md"), markdown.as_bytes())?;
        write(
            &representation.with_extension("txt"),
            outputs::text_item(&ctx).as_bytes(),
        )?;
        write(
            &representation.with_extension("rst"),
            outputs::rst_item(&ctx).as_bytes(),
        )?;
        write(
            &representation.with_extension("json"),
            outputs::item_json(&site, &build_ctx, &ctx, &published_body)?.as_bytes(),
        )?;
        Ok(site
            .absolute(&ctx.url)
            .map(|url| outputs::SitemapUrl::new(url, Some(archive_modified_at(&ctx)))))
    })?;
    sitemap_urls.extend(article_sitemaps.into_iter().flatten());
    phase("article pages and representations");

    for (previous, target) in duplicate_redirects {
        let target = site.url(&target);
        write(
            &out.join(previous).join("index.html"),
            outputs::redirect_stub(&site, &target).as_bytes(),
        )?;
    }

    let search_documents: Vec<_> = archive_items
        .iter()
        .zip(&prepared_bodies)
        .map(|(item, prepared)| pagefind::SearchDocument::new(item, prepared.plain_text()))
        .collect();

    let stubs = 4;

    let root_feed_items = feed_items(&river_items, &prepared_by_path, per_page);
    let atom = outputs::atom_feed(&site, &build_ctx, &root_feed_items);
    write(&out.join("feed.xml"), atom.as_bytes())?;
    write(&out.join("atom.xml"), atom.as_bytes())?;
    write(
        &out.join("rss.xml"),
        outputs::rss_collection(&site, &build_ctx, &site.title, "", &root_feed_items).as_bytes(),
    )?;
    write(
        &out.join("feed.json"),
        outputs::json_collection(&site, &site.title, "", &root_feed_items)?.as_bytes(),
    )?;
    write(
        &out.join("aggr.json"),
        outputs::instance_descriptor(&site, &build_ctx, archive_updated.unwrap_or(build_ctx.time))?
            .as_bytes(),
    )?;
    write(&out.join("llms.txt"), outputs::llms_txt(&site).as_bytes())?;
    write(
        &out.join("sources.opml"),
        outputs::sources_opml(
            &site,
            archive_updated.unwrap_or(build_ctx.time),
            &subscriptions,
        )
        .as_bytes(),
    )?;
    if let Some(root) = site.base_url.as_deref() {
        write(
            &out.join("linkset.json"),
            outputs::linkset_json(&site, &archive_items)?.as_bytes(),
        )?;
        let default_search_description = format!("Search {}", site.title);
        let search_description = if site.description.is_empty() {
            &default_search_description
        } else {
            &site.description
        };
        let search_url = format!("{root}?q={{searchTerms}}");
        let opensearch_url = format!("{root}opensearch.xml");
        let opensearch = outputs::opensearch_description(&outputs::OpenSearchDescription {
            short_name: &site.title,
            description: search_description,
            search_url: &search_url,
            self_url: Some(&opensearch_url),
        });
        write(&out.join("opensearch.xml"), opensearch.as_bytes())?;

        let mut robots = "User-agent: *\nAllow: /\n".to_string();
        if site.indexing {
            sitemap_urls.sort_by(|left, right| left.loc.cmp(&right.loc));
            sitemap_urls.dedup_by(|left, right| {
                if left.loc != right.loc {
                    return false;
                }
                right.lastmod = left.lastmod.max(right.lastmod);
                true
            });
            let sitemap_url = format!("{root}sitemap.xml");
            let sitemap = outputs::sitemap(
                &sitemap_urls,
                &sitemap_url,
                outputs::SitemapLimits::default(),
            )?;
            write(&out.join("sitemap.xml"), sitemap.root_xml().as_bytes())?;
            for chunk in sitemap.chunks() {
                write(&out.join(&chunk.name), chunk.xml.as_bytes())?;
            }
            robots.push_str(&format!("Sitemap: {sitemap_url}\n"));
        }
        if site.base_path == "/" {
            write(&out.join("robots.txt"), robots.as_bytes())?;
        }
    }
    write(&out.join(".nojekyll"), b"")?;
    write(&out.join(MARKER), env!("CARGO_PKG_VERSION").as_bytes())?;
    if let Some(domain) = cname(info) {
        write(&out.join("CNAME"), domain.as_bytes())?;
    }
    let assets = renderer.write_static(out)?;
    phase("feeds and static assets");

    pagefind::build_cached(
        out,
        &search_documents,
        &site.language,
        info.pagefind_cache.as_deref(),
    )?;
    phase("search index");

    if config.site.pwa {
        let offline_items = &archive_items[..archive_items
            .len()
            .min(config.site.preferences.offline_items)];
        write(
            &out.join("offline.html"),
            pages_ctx
                .simple(SimplePage {
                    page_items: Some(offline_items),
                    ..SimplePage::new("offline", "Offline", "offline.html", "offline.html")
                })?
                .as_bytes(),
        )?;
        write(
            &out.join("manifest.webmanifest"),
            pages_ctx
                .simple(SimplePage::new(
                    "manifest",
                    &config.site.title,
                    "manifest.webmanifest",
                    "manifest.webmanifest",
                ))?
                .as_bytes(),
        )?;
        // Only the shell: the reader caches the pages it actually opens.
        let paths = assets::precache_paths("", &assets);
        let mut sw_ctx = SwCtx {
            site: &site,
            build: &build_ctx,
            version: cache_version(&build_ctx),
            precache: assets::precache_entries(out, paths, &written_assets)?,
        };
        // Include the rendered worker and every resource revision so an installation never
        // deletes a live precache when only the theme or worker implementation changed.
        let worker = renderer.render("sw.js", &sw_ctx)?;
        sw_ctx.version = crate::model::sha1_hex(worker.as_bytes());
        let sw = renderer.render("sw.js", &sw_ctx)?;
        write(&out.join("sw.js"), sw.as_bytes())?;
        pages += 1;
    }

    phase("offline catalog and worker");
    let summary = Summary {
        pages,
        items: archive_items.len(),
        stubs,
    };
    let total = started.elapsed();
    log::debug!("build complete: {:.3}s", total.as_secs_f64());
    let report = build_report(&phases, total, summary);
    log::info!("{report}");
    // Debug logs are never enabled in the publish workflow; a notice reaches the run summary.
    if std::env::var_os("GITHUB_ACTIONS").is_some() {
        println!("::notice title=aggr build::{report}");
    }
    Ok(summary)
}

/// One line with every phase's wall time and the totals, so a slow publish run shows where the
/// time went. Phase names are the ones `docs/performance.md` reports against.
fn build_report(
    phases: &[(&str, std::time::Duration)],
    total: std::time::Duration,
    summary: Summary,
) -> String {
    let timings: Vec<String> = phases
        .iter()
        .map(|(name, elapsed)| format!("{name} {:.1}s", elapsed.as_secs_f64()))
        .chain(std::iter::once(format!(
            "total {:.1}s",
            total.as_secs_f64()
        )))
        .collect();
    format!(
        "build: {} ({} pages, {} items)",
        timings.join(", "),
        summary.pages,
        summary.items
    )
}

/// Template/static lookup order: the project's own `templates/`+`static/`, then the configured
/// theme directory, then the embedded default theme.
pub fn theme_layers(config: &Config, project_root: &Path) -> Result<Layers> {
    let mut layers = Layers::default();
    if project_root.join("templates").is_dir() || project_root.join("static").is_dir() {
        layers.dirs.push(project_root.to_path_buf());
    }
    match config.site.theme.as_str() {
        "default" => {
            // A development binary reads the shipped theme from its source tree so `aggr dev`
            // can rebuild template/CSS/JS edits without recompiling Rust. Release binaries stay
            // fully embedded and have no dependency on the build machine.
            #[cfg(debug_assertions)]
            {
                let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("themes/default");
                if source.is_dir() && !source.starts_with(project_root) {
                    layers.dirs.push(source);
                }
            }
        }
        theme => {
            let dir = project_root.join(theme);
            if !dir.is_dir() {
                bail!("theme {theme:?} is not a directory (git themes arrive in a later release)");
            }
            layers.dirs.push(dir);
        }
    }
    Ok(layers)
}

/// The host `CNAME` should carry: a release build on a domain that is not GitHub's own.
pub fn cname(info: &BuildInfo) -> Option<String> {
    if !info.release {
        return None;
    }
    let url = url::Url::parse(info.base_url.as_deref()?).ok()?;
    let host = url.host_str()?;
    (!host.ends_with(".github.io") && host != "localhost").then(|| host.to_string())
}

fn ensure_trailing_slash(url: &str) -> String {
    if url.ends_with('/') {
        url.to_string()
    } else {
        format!("{url}/")
    }
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Status;
    use chrono::TimeZone;

    fn day(d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, d, 0, 0, 0).unwrap()
    }

    #[test]
    fn retained_sources_keep_metadata_and_archive_pages_after_config_removal() {
        let dir = tempfile::tempdir().unwrap();
        let (config, _, store) = fixture(dir.path(), 2, "max_age_days = 30\npwa = false\n");
        store
            .write_source_state(
                "blog",
                &crate::store::SourceState {
                    title: Some("Hacker News: Best".into()),
                    site_url: Some("https://news.ycombinator.com/".into()),
                    resolved_url: Some("https://hnrss.org/best".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let items = store.items().unwrap();
        let contexts = source_contexts(&[], &store, &Status::default(), &items).unwrap();
        assert_eq!(contexts.len(), 1);
        let source = &contexts[0];
        assert_eq!(source.slug, "blog");
        assert_eq!(source.name, "Hacker News: Best");
        assert_eq!(source.url.as_deref(), Some("https://hnrss.org/best"));
        assert_eq!(
            source.site_url.as_deref(),
            Some("https://news.ycombinator.com/")
        );
        assert_eq!(source.count, 2);
        assert_eq!(source.latest, Some(day(2)));
        assert_eq!(source.page, "sources/blog/");

        let out = dir.path().join("out");
        build(&config, &[], &store, dir.path(), &info(out.clone())).unwrap();
        let archive = std::fs::read_to_string(out.join("sources/hnrss.org/index.html")).unwrap();
        assert!(archive.contains("Post 0"));
        assert!(archive.contains("Post 1"));
        assert!(archive.contains("Hacker News: Best"));
        let article =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(
            article.contains("href=\"../../../?q=source%3A%22hnrss.org%22\""),
            "{article}"
        );
        assert!(article.contains("title=\"Hacker News: Best\">hnrss.org</a>"));
        let archive_document = scraper::Html::parse_document(&archive);
        assert!(
            archive_document
                .select(&scraper::Selector::parse(".row .tags").unwrap())
                .next()
                .is_none()
        );
        for (html, root) in [(&archive, "../../"), (&article, "../../../")] {
            let document = scraper::Html::parse_document(html);
            let selector = scraper::Selector::parse(".meta .domain").unwrap();
            let source_link = document.select(&selector).next().unwrap();
            assert_eq!(
                source_link
                    .select(&scraper::Selector::parse("a.source-feed").unwrap())
                    .next()
                    .unwrap()
                    .value()
                    .attr("href"),
                Some(format!("{root}?q=source%3A%22hnrss.org%22").as_str())
            );
            assert_eq!(
                source_link.text().collect::<String>().trim(),
                "blog.example via hnrss.org"
            );
            let emphasis = scraper::Selector::parse("em").unwrap();
            assert_eq!(
                source_link
                    .select(&emphasis)
                    .next()
                    .unwrap()
                    .text()
                    .collect::<String>(),
                "via hnrss.org"
            );
        }
    }

    /// Page-relative links reduced to their site-relative form, so pages at different depths
    /// can be compared.
    fn site_relative_links(html: &str) -> String {
        html.replace("href=\"../../../", "href=\"")
            .replace("href=\"./", "href=\"")
    }

    /// One page-relative attribute value reduced to its site-relative form.
    fn site_relative(value: Option<&str>) -> Option<&str> {
        value.map(|value| value.trim_start_matches("../").trim_start_matches("./"))
    }

    #[test]
    fn feed_and_article_share_metadata_including_sources() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "");
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let parse = |path: &str| {
            scraper::Html::parse_document(&std::fs::read_to_string(out.join(path)).unwrap())
        };
        let feed = parse("index.html");
        let article = parse("items/blog/2026-09-01-post-0/index.html");
        let fields = scraper::Selector::parse(".meta > .meta-field").unwrap();
        let source = scraper::Selector::parse(".domain").unwrap();
        // Links walk up to the site root from wherever the page sits; everything else is identical.
        let shared = |document: &scraper::Html| {
            document
                .select(&fields)
                .map(|field| site_relative_links(&field.inner_html()))
                .collect::<Vec<_>>()
        };
        assert_eq!(shared(&feed), shared(&article));
        assert_eq!(feed.select(&source).count(), 1);
        assert_eq!(article.select(&source).count(), 1);
        assert!(
            feed.select(&scraper::Selector::parse(".reading-stats").unwrap())
                .next()
                .is_some()
        );
    }

    #[test]
    fn retained_sources_without_state_keep_archives_without_invented_urls() {
        let dir = tempfile::tempdir().unwrap();
        let (_, _, store) = fixture(dir.path(), 1, "");
        let contexts =
            source_contexts(&[], &store, &Status::default(), &store.items().unwrap()).unwrap();
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0].name, "Unknown source");
        assert!(contexts[0].url.is_none());
        assert!(contexts[0].site_url.is_none());
    }

    #[test]
    fn window_respects_count_and_age() {
        let dates = vec![day(10), day(9), day(8), day(1)];
        let w = window(&dates, day(10), 2, 30);
        assert_eq!(
            w,
            Window {
                rendered: vec![0, 1],
            }
        );
        let w = window(&dates, day(10), 10, 5);
        assert_eq!(
            w,
            Window {
                rendered: vec![0, 1, 2],
            }
        );
    }

    #[test]
    fn pagination_paths() {
        let pages = paginate("", 0, 10);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].path, "");
        assert_eq!(pages[0].range, 0..0);
        assert_eq!(pages[0].context.first, "./");
        assert_eq!(pages[0].context.last, "./");
        assert_eq!(pages[0].context.total_items, 0);
        let pages = paginate("sources/x/", 25, 10);
        assert_eq!(
            pages.iter().map(|p| p.path.as_str()).collect::<Vec<_>>(),
            ["sources/x/", "sources/x/page/2/", "sources/x/page/3/"]
        );
        assert_eq!(pages[2].range, 20..25);
    }

    #[test]
    fn paginator_context_uses_stable_edge_and_adjacent_routes() {
        let pages = paginate("", 35, 10);
        let first = &pages[0].context;
        assert_eq!(first.current_index, 1);
        assert_eq!(first.number_pagers, 4);
        assert_eq!(first.paginate_by, 10);
        assert_eq!(first.total_items, 35);
        assert_eq!(first.first, "./");
        assert_eq!(first.last, "page/4/");
        assert_eq!(first.previous, None);
        assert_eq!(first.next.as_deref(), Some("page/2/"));

        let middle = &pages[2].context;
        assert_eq!(middle.first, "./");
        assert_eq!(middle.last, "page/4/");
        assert_eq!(middle.previous.as_deref(), Some("page/2/"));
        assert_eq!(middle.next.as_deref(), Some("page/4/"));
        assert_eq!(middle.offset, 20);

        let last = &pages[3].context;
        assert_eq!(last.previous.as_deref(), Some("page/3/"));
        assert_eq!(last.next, None);

        let nested = paginate("sources/x/", 11, 10);
        assert_eq!(nested[1].context.first, "sources/x/");
        assert_eq!(nested[1].context.previous.as_deref(), Some("sources/x/"));
        assert_eq!(nested[1].context.last, "sources/x/page/2/");
    }

    #[test]
    fn base_paths() {
        assert_eq!(base_path(None), "/");
        assert_eq!(base_path(Some("https://u.github.io/")), "/");
        assert_eq!(base_path(Some("https://u.github.io/repo")), "/repo/");
        assert_eq!(base_path(Some("https://u.github.io/a/b/")), "/a/b/");
    }

    #[test]
    fn relative_roots_cover_files_and_nested_routes() {
        assert_eq!(relative_root(""), "./");
        assert_eq!(relative_root("404.html"), "./");
        assert_eq!(relative_root("search/"), "../");
        assert_eq!(relative_root("categories/rust/"), "../../");
        assert_eq!(relative_root("sources/rust/page/2/"), "../../../../");
    }

    #[test]
    fn cache_version_changes_with_content_but_not_rebuild_time() {
        let build = BuildCtx {
            time: day(2),
            version: "0".into(),
            app_version: "app".into(),
            content_version: "content".into(),
            config_sha: Some("c".repeat(40)),
            data_sha: None,
            generation: "g".repeat(40),
            release: false,
        };
        assert_eq!(
            cache_version(&build),
            format!("0-local-{}-{}", "c".repeat(12), "g".repeat(12))
        );
        let later = BuildCtx {
            data_sha: Some("d".repeat(40)),
            time: day(3),
            ..build.clone()
        };
        assert_ne!(cache_version(&later), cache_version(&build));
        let rebuilt = BuildCtx {
            time: day(3),
            ..build.clone()
        };
        assert_eq!(cache_version(&rebuilt), cache_version(&build));
    }

    #[test]
    fn build_report_lists_phases_totals_and_counts() {
        let phases = [
            (
                "archive preparation",
                std::time::Duration::from_millis(2_040),
            ),
            ("search index", std::time::Duration::from_millis(14_160)),
        ];
        let summary = Summary {
            pages: 12,
            items: 3,
            stubs: 4,
        };
        assert_eq!(
            build_report(&phases, std::time::Duration::from_millis(52_010), summary),
            "build: archive preparation 2.0s, search index 14.2s, total 52.0s (12 pages, 3 items)"
        );
    }

    #[test]
    fn render_generation_changes_only_at_visible_time_boundaries() {
        let at = day(3);
        let item = |path: &str, published| Item {
            path: path.into(),
            front: crate::model::FrontMatter {
                title: path.into(),
                link: format!("https://example.com/{path}"),
                source: "example".into(),
                published: Some(published),
                first_seen: published,
                ..Default::default()
            },
            body: "body".into(),
        };
        let site = SiteConfig {
            max_age_days: 2,
            ..Default::default()
        };

        let stable = item("stable", at - Duration::hours(30));
        assert_eq!(
            render_generation(std::slice::from_ref(&stable), &site, at),
            render_generation(&[stable], &site, at + Duration::minutes(10))
        );

        let age_boundary = item("age", at - Duration::minutes(59));
        assert_ne!(
            render_generation(std::slice::from_ref(&age_boundary), &site, at),
            render_generation(&[age_boundary], &site, at + Duration::minutes(2))
        );

        let cutoff = item("cutoff", at - Duration::hours(47) - Duration::minutes(59));
        assert_ne!(
            render_generation(std::slice::from_ref(&cutoff), &site, at),
            render_generation(&[cutoff], &site, at + Duration::minutes(2))
        );
        let media_cutoff = item("media", at - Duration::days(30));
        assert_ne!(
            render_generation(std::slice::from_ref(&media_cutoff), &site, at),
            render_generation(&[media_cutoff], &site, at + Duration::seconds(1))
        );
    }

    /// Hand-edited front matter may hold YAML that JSON cannot carry. Such items must still take
    /// part in the render fingerprint and must never abort `content_version`.
    #[test]
    fn exotic_front_matter_extra_still_fingerprints_and_builds() {
        use crate::model::{FrontMatter, file_stem, item_dir};
        use crate::store::NewItem;

        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "");
        let exotic = |note: &str| {
            let mut junk = serde_yaml_ng::Mapping::new();
            junk.insert(serde_yaml_ng::Value::Null, "x".into());
            BTreeMap::from([
                ("junk".to_string(), serde_yaml_ng::Value::Mapping(junk)),
                ("ratio".to_string(), serde_yaml_ng::Value::from(f64::NAN)),
                ("note".to_string(), note.into()),
            ])
        };
        let date = day(10);
        for note in ["a", "b"] {
            let front = FrontMatter {
                title: format!("Exotic {note}"),
                link: format!("https://blog.example/exotic-{note}"),
                source: "blog".into(),
                published: Some(date),
                first_seen: date,
                extra: exotic(note),
                ..Default::default()
            };
            store
                .write_item(NewItem {
                    dir: &item_dir("blog", date),
                    stem: &file_stem(date, &front.title),
                    front: &front,
                    body: "Hello",
                    html: None,
                    preview: None,
                    images: &[],
                })
                .unwrap();
        }

        let mut items = store.items().unwrap();
        items.retain(|item| item.front.title.starts_with("Exotic"));
        items.sort_by(|a, b| a.path.cmp(&b.path));
        let [first, second] = items.as_mut_slice() else {
            panic!("expected two exotic items, got {}", items.len());
        };
        // Same path and body: only the readable part of `extra` differs.
        second.path = first.path.clone();
        assert_ne!(
            render_generation(std::slice::from_ref(first), &config.site, day(20)),
            render_generation(std::slice::from_ref(second), &config.site, day(20)),
        );

        let build_info = info(dir.path().join("out"));
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
    }

    /// Every cache the publish workflow keeps (media receipts, search index, service-worker
    /// precache) assumes identical inputs give identical bytes, so nothing but the archive may
    /// leak into the output. The fixture dates are absolute and days away from every age band and
    /// river cutoff, so an hour of wall-clock drift between the builds changes no visible state.
    /// The search index goes through the same input-keyed cache the workflow uses: Pagefind 1.5.2
    /// encodes filter values in hash-map order (see the TODO in its `index/mod.rs`), so a cold
    /// index is not byte-stable under load and cannot be part of this guarantee.
    #[test]
    fn identical_archives_build_byte_identical_trees_regardless_of_wall_clock() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 3, "pwa = true\n");
        let pagefind_cache = dir.path().join("pagefind-cache");
        let tree = |name: &str, now: DateTime<Utc>| {
            let out = dir.path().join(name);
            let mut build_info = info(out.clone());
            build_info.release = true;
            build_info.now = now;
            build_info.pagefind_cache = Some(pagefind_cache.clone());
            build(&config, &sources, &store, dir.path(), &build_info).unwrap();
            output_digests(&out)
        };
        let first = tree("first", day(20));
        let second = tree("second", day(20) + Duration::hours(1));
        assert_eq!(
            first.keys().collect::<Vec<_>>(),
            second.keys().collect::<Vec<_>>()
        );
        for (path, digest) in &first {
            assert_eq!(
                &second[path],
                digest,
                "{} differs between two builds of the same archive",
                path.display()
            );
        }
    }

    /// Relative path → SHA-1 of every regular file below `out`.
    fn output_digests(out: &Path) -> BTreeMap<PathBuf, String> {
        walkdir::WalkDir::new(out)
            .into_iter()
            .map(Result::unwrap)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| {
                let relative = entry.path().strip_prefix(out).unwrap().to_path_buf();
                let digest = crate::model::sha1_hex(std::fs::read(entry.path()).unwrap());
                (relative, digest)
            })
            .collect()
    }

    /// `path` → content hash for every content-addressed media file this build published.
    fn published_media(out: &Path) -> BTreeMap<String, String> {
        ["assets/images", "assets/previews", "assets/documents"]
            .iter()
            .flat_map(|dir| walkdir::WalkDir::new(out.join(dir)))
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| {
                let relative = entry
                    .path()
                    .strip_prefix(out)
                    .expect("under the output root")
                    .to_string_lossy()
                    .replace('\\', "/");
                let digest = crate::model::sha1_hex(std::fs::read(entry.path()).unwrap());
                (relative, digest)
            })
            .collect()
    }

    /// The media phase gathers on worker threads and publishes in item order. Whatever the
    /// worker count, the dedupe map, both memos and therefore every output byte must agree. The
    /// fixture repeats one image across items and gives another item a stored preview, so shared
    /// paths, derived thumbnails and placeholders all take part; ten items make the first window
    /// large enough for `parallel::map` to spread across threads.
    #[test]
    fn worker_count_never_changes_the_output_tree() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 10, "pwa = true\n");
        let diagram = |name: &str, color: [u8; 4]| {
            let pixels = image::RgbaImage::from_pixel(640, 320, image::Rgba(color));
            let mut encoded = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(pixels)
                .write_to(&mut encoded, image::ImageFormat::Png)
                .unwrap();
            crate::media::prepare_asset(
                &crate::media::Candidate {
                    url: url::Url::parse(&format!("https://blog.example/{name}.png")).unwrap(),
                    alt: Some(name.into()),
                },
                encoded.into_inner(),
                &crate::media::MediaLimits::default(),
            )
            .unwrap()
        };
        let shared = diagram("shared", [21, 82, 109, 255]);
        let distinct = diagram("distinct", [200, 40, 40, 255]);
        let mut items = store.items().unwrap();
        items.sort_by(|a, b| a.path.cmp(&b.path));
        for (index, item) in items.iter_mut().enumerate() {
            let (directory, stem) = item.path.rsplit_once('/').unwrap();
            let images = match index % 3 {
                0 => vec![shared.clone()],
                1 => vec![distinct.clone(), shared.clone()],
                _ => Vec::new(),
            };
            item.body = images
                .iter()
                .map(|asset| format!("![Diagram]({})\n\n", asset.source_url))
                .chain(std::iter::once("Hello".to_string()))
                .collect();
            item.front.images = images.iter().map(|asset| asset.metadata(stem)).collect();
            let preview = (index == 2)
                .then(|| crate::preview::thumbnail(&distinct.master_bytes, None).unwrap());
            item.front.preview = preview.as_ref().map(|thumbnail| thumbnail.metadata(stem));
            store
                .write_item(crate::store::NewItem {
                    dir: directory,
                    stem,
                    front: &item.front,
                    body: &item.body,
                    html: None,
                    preview: preview.as_ref().map(|thumbnail| thumbnail.bytes.as_slice()),
                    images: &images,
                })
                .unwrap();
        }
        let pagefind_cache = dir.path().join("pagefind-cache");
        let tree = |name: &str, workers: usize| {
            let out = dir.path().join(name);
            let mut build_info = info(out.clone());
            build_info.release = true;
            build_info.pagefind_cache = Some(pagefind_cache.clone());
            let started = std::time::Instant::now();
            parallel::with_workers(workers, || {
                build(&config, &sources, &store, dir.path(), &build_info)
            })
            .unwrap();
            println!(
                "{name} build with {workers} worker(s): {:?}",
                started.elapsed()
            );
            output_digests(&out)
        };
        let serial = tree("serial", 1);
        let threaded = tree("threaded", 4);
        assert_eq!(
            serial.keys().collect::<Vec<_>>(),
            threaded.keys().collect::<Vec<_>>()
        );
        for (path, digest) in &serial {
            assert_eq!(
                &threaded[path],
                digest,
                "{} differs between a serial and a parallel build",
                path.display()
            );
        }
        let published = |prefix: &str| {
            serial
                .keys()
                .filter(|path| path.starts_with(prefix))
                .count()
        };
        assert!(
            published("assets/images") >= 2,
            "shared and distinct images"
        );
        assert!(
            published("assets/previews") >= 2,
            "stored and derived previews"
        );
    }

    /// A store with `count` items of one source, newest last, and a matching config.
    fn fixture(root: &Path, count: usize, extra: &str) -> (Config, Vec<Source>, Store) {
        use crate::model::{FrontMatter, file_stem, item_dir};
        use crate::store::NewItem;

        let config = Config::parse(&format!(
            "[site]\ntitle = \"Demo <site>\"\n{extra}\n[[sources]]\nslug = \"blog\"\nurl = \"https://blog.example/feed\"\n"
        ))
        .unwrap();
        let sources = config.resolve_sources(&|_| None).unwrap();
        let store = Store::open(root.join("data"));
        for i in 0..count {
            let date = day(1 + i as u32);
            let front = FrontMatter {
                title: format!("Post {i}"),
                link: format!("https://blog.example/{i}"),
                source: "blog".into(),
                published: Some(date),
                first_seen: date,
                ..Default::default()
            };
            store
                .write_item(NewItem {
                    dir: &item_dir("blog", date),
                    stem: &file_stem(date, &front.title),
                    front: &front,
                    body: "Hello",
                    html: None,
                    preview: None,
                    images: &[],
                })
                .unwrap();
        }
        (config, sources, store)
    }

    fn info(out: PathBuf) -> BuildInfo {
        BuildInfo {
            out,
            base_url: Some("https://u.github.io/repo/".into()),
            config_sha: Some("c".repeat(40)),
            config_path: Some("aggr.toml".into()),
            data_sha: Some("d".repeat(40)),
            generation: "fixture".into(),
            now: day(20),
            release: false,
            development: false,
            render_cache_key: None,
            discussions: crate::discussions::ResolutionSet::default(),
            pagefind_cache: None,
        }
    }

    #[test]
    fn build_budget_preserves_text_and_archives_while_prioritizing_recent_media() {
        let dir = tempfile::tempdir().unwrap();
        let (mut config, sources, store) = fixture(dir.path(), 2, "pwa = true\n");
        let mut images = Vec::new();
        let mut items = store.items().unwrap();
        items.sort_by(|a, b| a.path.cmp(&b.path));
        for (index, item) in items.iter_mut().enumerate() {
            let mut random = 42u32 + index as u32;
            let pixels = image::RgbImage::from_fn(800, 600, |_, _| {
                random ^= random << 13;
                random ^= random >> 17;
                random ^= random << 5;
                image::Rgb([random as u8, (random >> 8) as u8, (random >> 16) as u8])
            });
            let mut encoded = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgb8(pixels)
                .write_to(&mut encoded, image::ImageFormat::Png)
                .unwrap();
            let asset = crate::media::prepare_asset(
                &crate::media::Candidate {
                    url: url::Url::parse(&format!("https://blog.example/image-{index}.png"))
                        .unwrap(),
                    alt: Some("Illustration".into()),
                },
                encoded.into_inner(),
                &crate::media::MediaLimits::default(),
            )
            .unwrap();
            if index == 0 {
                item.front.published = Some(day(1) - Duration::days(40));
            }
            item.body = format!(
                "Distinct article {index} remains readable.\n\n![Illustration]({})",
                asset.source_url
            );
            let (directory, stem) = item.path.rsplit_once('/').unwrap();
            item.front.images = vec![asset.metadata(stem)];
            store
                .write_item(crate::store::NewItem {
                    dir: directory,
                    stem,
                    front: &item.front,
                    body: &item.body,
                    html: None,
                    preview: None,
                    images: std::slice::from_ref(&asset),
                })
                .unwrap();
            images.push(asset);
        }
        let archive_before = output_digests(&dir.path().join("data"));
        let mut build_info = info(dir.path().join("out"));
        build_info.pagefind_cache = Some(dir.path().join("cache"));
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let master = |asset: &crate::media::Asset| {
            format!(
                "assets/images/{}.{}",
                asset.master_hash, asset.master_extension
            )
        };
        assert!(
            !build_info.out.join(master(&images[0])).exists(),
            "old master is compressed"
        );
        assert_eq!(
            std::fs::read(build_info.out.join(master(&images[1]))).unwrap(),
            images[1].master_bytes
        );
        let full_bytes = budget::output_bytes(&build_info.out).unwrap();
        let compressed = compressed_media::compact_cached(
            images[0].clone(),
            Some(
                &crate::cache::Namespace::DeploymentMedia
                    .dir(build_info.pagefind_cache.as_ref().unwrap()),
            ),
        )
        .unwrap();
        assert!(compressed.master_bytes.len() < images[0].master_bytes.len());
        assert!(build_info.out.join(master(&compressed)).is_file());
        let full_catalog = published_media(&build_info.out);
        assert!(
            full_catalog
                .keys()
                .any(|path| path.contains("assets/images/"))
        );

        config.site.build_max_bytes = full_bytes - compressed.master_bytes.len() as u64 / 2;
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        assert!(budget::output_bytes(&build_info.out).unwrap() <= config.site.build_max_bytes);
        assert_eq!(
            std::fs::read(build_info.out.join(master(&images[1]))).unwrap(),
            images[1].master_bytes
        );
        assert!(
            !build_info.out.join(master(&compressed)).exists(),
            "older media yields to recent media"
        );

        build_once(
            &config,
            &sources,
            &store,
            dir.path(),
            &build_info,
            &mut budget::MediaBudget::new(0),
        )
        .unwrap();
        let core_bytes = budget::output_bytes(&build_info.out).unwrap();
        config.site.build_max_bytes = core_bytes + 64_000;
        assert!(config.site.build_max_bytes < full_bytes);
        let summary = build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        assert_eq!(summary.items, 2);
        assert!(budget::output_bytes(&build_info.out).unwrap() <= config.site.build_max_bytes);
        assert!(!build_info.out.join(master(&images[1])).exists());
        for (index, item) in items.iter().enumerate() {
            let url = context::item_url(&item.path);
            let representation = build_info.out.join(url.trim_end_matches('/'));
            for path in [
                representation.join("index.html"),
                representation.with_extension("md"),
                representation.with_extension("txt"),
            ] {
                let text = std::fs::read_to_string(path).unwrap();
                assert!(text.contains(&format!("Distinct article {index} remains readable.")));
            }
            let html = std::fs::read_to_string(representation.join("index.html")).unwrap();
            assert!(
                html.contains(&images[index].source_url),
                "omitted images retain publisher URLs"
            );
        }
        for path in published_media(&build_info.out).keys() {
            assert!(
                build_info.out.join(path).is_file(),
                "missing published resource {path}"
            );
        }
        assert_eq!(output_digests(&dir.path().join("data")), archive_before);
    }

    #[test]
    fn build_budget_includes_the_cache_marker_at_the_exact_limit() {
        let dir = tempfile::tempdir().unwrap();
        let (mut config, sources, store) = fixture(dir.path(), 1, "");
        let mut build_info = info(dir.path().join("out"));
        build_info.pagefind_cache = Some(dir.path().join("cache"));
        build_info.render_cache_key = Some("a".repeat(40));
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        assert_eq!(
            std::fs::read_to_string(build_info.out.join(crate::cache::RENDER_KEY_FILE)).unwrap(),
            "a".repeat(40)
        );
        let exact = budget::output_bytes(&build_info.out).unwrap();
        config.site.build_max_bytes = exact;
        let summary = build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        crate::cache::store_render(
            build_info.pagefind_cache.as_ref().unwrap(),
            build_info.render_cache_key.as_ref().unwrap(),
            &build_info.out,
            summary,
        )
        .unwrap();
        crate::cache::restore_render(
            build_info.pagefind_cache.as_ref().unwrap(),
            build_info.render_cache_key.as_ref().unwrap(),
            &build_info.out,
        )
        .unwrap()
        .unwrap();
        assert_eq!(budget::output_bytes(&build_info.out).unwrap(), exact);
        verify_output_budget(&build_info.out, exact).unwrap();
        std::fs::write(build_info.out.join("extra"), b"x").unwrap();
        assert!(verify_output_budget(&build_info.out, exact).is_err());
        config.site.build_max_bytes = exact - 1;
        let error = build(&config, &sources, &store, dir.path(), &build_info).unwrap_err();
        assert!(error.to_string().contains("text and required assets"));
    }

    #[test]
    fn build_rejects_text_and_reader_larger_than_budget() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "build_max_bytes = 1\n");
        let out = dir.path().join("out");
        let error = build(&config, &sources, &store, dir.path(), &info(out)).unwrap_err();
        assert!(format!("{error:#}").contains("text and required assets"));
        assert_eq!(store.items().unwrap().len(), 1);
    }

    #[test]
    fn publisher_and_feed_share_one_article_with_separate_source_memberships() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::parse(
            r#"
[site]
pwa = false
[[sources]]
slug = "lobsters"
name = "Lobsters feed"
url = "https://lobste.rs/rss"
[[sources]]
slug = "duckdb"
name = "DuckDB"
url = "https://duckdb.org/feed.xml"
[[sources]]
slug = "duckdb-news"
name = "DuckDB news"
url = "https://duckdb.org/news.xml"
"#,
        )
        .unwrap();
        let sources = config.resolve_sources(&|_| None).unwrap();
        let store = Store::open(dir.path().join("data"));
        for (source, stem, title, link) in [
            (
                "lobsters",
                "duckdb-copy",
                "DuckDB article",
                "https://duckdb.org/2026/article",
            ),
            (
                "duckdb",
                "duckdb-original",
                "DuckDB article",
                "https://duckdb.org/2026/article#discussion",
            ),
            (
                "lobsters",
                "maharship",
                "Maharship article",
                "https://maharship.com/posts/example",
            ),
        ] {
            store
                .write_item(crate::store::NewItem {
                    dir: &format!("items/{source}/2026/09"),
                    stem,
                    front: &crate::model::FrontMatter {
                        source: source.into(),
                        title: title.into(),
                        link: link.into(),
                        first_seen: day(1),
                        ..Default::default()
                    },
                    body: title,
                    html: None,
                    preview: None,
                    images: &[],
                })
                .unwrap();
        }
        let out = dir.path().join("out");
        let summary = build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        assert_eq!(summary.items, 2, "one canonical item per original URL");
        let feed = std::fs::read_to_string(out.join("sources/lobste.rs/index.html")).unwrap();
        assert!(
            feed.contains("DuckDB article"),
            "deduplication retains the feed membership"
        );
        let article =
            std::fs::read_to_string(out.join("items/duckdb/duckdb-original/index.html")).unwrap();
        let parsed = scraper::Html::parse_document(&article);
        let links: Vec<_> = parsed
            .select(&scraper::Selector::parse(".itemhead .meta .domain a").unwrap())
            .collect();
        assert_eq!(
            links.len(),
            2,
            "publisher and distinct feed hosts are separate links"
        );
        assert!(
            links[0]
                .value()
                .attr("href")
                .unwrap()
                .contains("source%3A%22duckdb.org%22")
        );
        assert!(
            links[1]
                .value()
                .attr("href")
                .unwrap()
                .contains("source%3A%22lobste.rs%22")
        );
        assert!(
            links
                .iter()
                .all(|link| !link.text().collect::<String>().contains("via"))
        );
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("search-catalog.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["docs"], 2);
        let facets = manifest["facets"]["source"].as_array().unwrap();
        let subscriptions = std::fs::read_to_string(out.join("sources.opml")).unwrap();
        assert!(subscriptions.contains("https://duckdb.org/feed.xml"));
        assert!(subscriptions.contains("https://duckdb.org/news.xml"));
        assert_eq!(
            facets
                .iter()
                .find(|facet| facet["value"] == "lobste.rs")
                .unwrap()["count"],
            2
        );
        assert_eq!(
            facets
                .iter()
                .find(|facet| facet["value"] == "duckdb.org")
                .unwrap()["count"],
            1
        );
        let publisher = facets
            .iter()
            .find(|facet| facet["label"] == "maharship.com")
            .expect("unconfigured publishers receive a source listing");
        assert!(
            out.join(format!(
                "sources/{}/index.html",
                publisher["value"].as_str().unwrap()
            ))
            .is_file()
        );
        let generated =
            std::fs::read_to_string(out.join("items/lobsters/maharship/index.html")).unwrap();
        let generated = scraper::Html::parse_document(&generated);
        let publisher_link = generated
            .select(&scraper::Selector::parse(".itemhead .meta .domain a").unwrap())
            .next()
            .unwrap();
        let publisher_href = publisher_link.value().attr("href").unwrap();
        assert!(publisher_href.contains("source%3A%22maharship.com%22"));
        assert!(!publisher_href.contains("publisher-"));
        assert_eq!(publisher["value"], "maharship.com");
        let browse = std::fs::read_to_string(out.join("sources/index.html")).unwrap();
        assert!(browse.contains("source%3A%22maharship.com%22"));
        assert!(browse.contains("source%3A%22lobste.rs%22"));
        assert_eq!(
            store.items().unwrap().len(),
            3,
            "build leaves stored captures intact"
        );
    }

    #[test]
    fn archive_selection_is_order_independent_and_keeps_anonymous_items() {
        let item = |path: &str, link: &str, body: &str| Item {
            path: path.into(),
            front: crate::model::FrontMatter {
                link: link.into(),
                ..Default::default()
            },
            body: body.into(),
        };
        let items = vec![
            item("items/hn/a", "https://example.test/article", "Summary"),
            item(
                "items/hn/b",
                "https://example.test/article#top",
                "Full article content",
            ),
            item(
                "items/hn/c",
                "https://example.test/article?id=1",
                "Distinct query",
            ),
            item("items/hn/d", "", "Anonymous first"),
            item("items/hn/e", "", "Anonymous second"),
        ];
        let (selected, redirects) = visible_archive(items.clone(), &[]);
        assert_eq!(selected.len(), 4);
        assert!(selected.iter().any(|item| item.path == "items/hn/b"));
        assert_eq!(
            redirects,
            vec![("items/hn/a/".into(), "items/hn/b/".into())]
        );
        let (mut reversed, reverse_redirects) =
            visible_archive(items.into_iter().rev().collect(), &[]);
        let mut selected = selected;
        selected.sort_by(|a, b| a.path.cmp(&b.path));
        reversed.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(selected, reversed);
        assert_eq!(redirects, reverse_redirects);
    }

    #[test]
    fn archived_duplicates_and_shorts_are_removed_from_all_collections() {
        use crate::model::{FrontMatter, file_stem, item_dir};
        use crate::store::NewItem;

        let dir = tempfile::tempdir().unwrap();
        let (_, _, store) = fixture(dir.path(), 0, "");
        let config = Config::parse(
            r#"[site]
max_age_days = 365
pwa = true
indexing = true
[[sources]]
slug = "hn"
url = "https://hnrss.org/frontpage"
category = "News"
[[sources]]
slug = "blog"
url = "https://blog.example/feed"
category = "Science"
"#,
        )
        .unwrap();
        let sources = config.resolve_sources(&|_| None).unwrap();
        let mut paths = Vec::new();
        for (source, title, link, hidden, labels) in [
            (
                "hn",
                "Duplicate headline",
                "http://www.blog.example/article/?utm_source=hn#top",
                false,
                vec!["duplicate-only"],
            ),
            (
                "blog",
                "Publisher headline",
                "https://blog.example/article",
                false,
                vec!["science"],
            ),
            (
                "hn",
                "Excluded short",
                "https://www.youtube.com/shorts/abc123",
                false,
                vec!["short-only"],
            ),
            (
                "hn",
                "Hidden headline",
                "https://blog.example/hidden",
                true,
                vec!["hidden-only"],
            ),
            (
                "hn",
                "Distinct article",
                "https://blog.example/article?id=2",
                false,
                vec![],
            ),
        ] {
            let date = day(19);
            let front = FrontMatter {
                title: title.into(),
                link: link.into(),
                source: source.into(),
                published: Some(date),
                first_seen: date,
                hidden,
                labels: labels.into_iter().map(str::to_owned).collect(),
                ..Default::default()
            };
            let item_dir = item_dir(source, date);
            let stem = file_stem(date, title);
            store
                .write_item(NewItem {
                    dir: &item_dir,
                    stem: &stem,
                    front: &front,
                    body: title,
                    html: None,
                    preview: None,
                    images: &[],
                })
                .unwrap();
            paths.push(context::item_url(&format!("{item_dir}/{stem}")));
        }
        let before = store.items().unwrap();
        let out = dir.path().join("out");
        let mut build_info = info(out.clone());
        build_info.release = true;
        let summary = build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        assert_eq!(summary.items, 2);
        assert_eq!(
            store.items().unwrap(),
            before,
            "rendering leaves the archive intact"
        );

        for path in [
            "index.html",
            "sources/hnrss.org/index.html",
            "sources/blog.example/index.html",
            "categories/news/index.html",
            "categories/science/index.html",
            "browse/index.html",
            "feed.xml",
            "atom.xml",
            "rss.xml",
            "feed.json",
            "linkset.json",
            "sitemap.xml",
            "sources/hnrss.org/feed.json",
            "sources/blog.example/atom.xml",
            "categories/news/rss.xml",
            "sw.js",
        ] {
            let text = std::fs::read_to_string(out.join(path)).unwrap();
            for excluded in ["Duplicate headline", "Excluded short", "Hidden headline"] {
                assert!(!text.contains(excluded), "{path} includes {excluded}");
            }
            for path in [&paths[0], &paths[2], &paths[3]] {
                assert!(!text.contains(path), "collection links to excluded {path}");
            }
        }
        assert!(!out.join("tags/short-only").exists());
        assert!(!out.join("tags/duplicate-only").exists());
        assert!(!out.join(&paths[2]).exists());
        assert!(!out.join(&paths[3]).exists());
        let redirect = std::fs::read_to_string(out.join(&paths[0]).join("index.html")).unwrap();
        assert!(redirect.contains(&paths[1]));
        assert!(redirect.contains("noindex"));
        assert!(out.join(&paths[1]).join("index.html").is_file());
    }

    #[test]
    fn old_items_stay_browsable_in_source_archives() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "max_age_days = 5\npwa = false\n");
        let out = dir.path().join("out");
        let summary = build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        assert_eq!(summary.items, 1);
        assert_eq!(summary.stubs, 4);

        let feed = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(
            !feed.contains("Post 0"),
            "old items should not crowd the recent feed"
        );
        let source = std::fs::read_to_string(out.join("sources/blog.example/index.html")).unwrap();
        assert!(source.contains("Post 0"), "{source}");
        assert!(
            out.join("items/blog/2026-09-01-post-0/index.html")
                .is_file()
        );
    }

    #[test]
    fn rendered_worker_is_stable_and_versions_theme_and_worker_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = true\n");
        let out = dir.path().join("out");
        let mut build_info = info(out.clone());
        build_info.release = true;
        build_info.pagefind_cache = Some(dir.path().join("pagefind-cache"));
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let original = std::fs::read_to_string(out.join("sw.js")).unwrap();
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("sw.js")).unwrap(),
            original
        );

        std::fs::create_dir(dir.path().join("static")).unwrap();
        std::fs::write(dir.path().join("static/style.css"), "body { color: teal; }").unwrap();
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let styled = std::fs::read_to_string(out.join("sw.js")).unwrap();
        let version = |worker: &str| {
            worker
                .lines()
                .find(|line| line.starts_with("var VERSION ="))
                .unwrap()
                .to_owned()
        };
        assert_ne!(version(&styled), version(&original));

        std::fs::create_dir(dir.path().join("templates")).unwrap();
        std::fs::write(
            dir.path().join("templates/sw.js"),
            format!(
                "{}\n// Worker revision test.\n",
                include_str!("../../themes/default/templates/sw.js")
            ),
        )
        .unwrap();
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let revised = std::fs::read_to_string(out.join("sw.js")).unwrap();
        assert_ne!(version(&revised), version(&styled));
    }

    #[test]
    fn application_fingerprint_tracks_effective_assets_templates_and_binary() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let theme = dir.path().join("theme");
        std::fs::create_dir_all(project.join("static")).unwrap();
        std::fs::create_dir_all(theme.join("static")).unwrap();
        std::fs::create_dir_all(project.join("templates")).unwrap();
        std::fs::write(project.join("static/style.css"), "body { color: teal; }").unwrap();
        std::fs::write(theme.join("static/style.css"), "body { color: red; }").unwrap();
        let layers = Layers {
            dirs: vec![project.clone(), theme.clone()],
        };
        let original = app_version("1.2.3", &layers).unwrap();
        assert_ne!(original, app_version("1.2.4", &layers).unwrap());

        std::fs::write(theme.join("static/style.css"), "body { color: blue; }").unwrap();
        assert_eq!(
            original,
            app_version("1.2.3", &layers).unwrap(),
            "shadowed layer bytes do not ship"
        );
        std::fs::write(project.join("static/style.css"), "body { color: green; }").unwrap();
        let styled = app_version("1.2.3", &layers).unwrap();
        assert_ne!(original, styled);
        std::fs::write(
            project.join("templates/custom-include.html"),
            "<p>Custom include</p>",
        )
        .unwrap();
        let template = app_version("1.2.3", &layers).unwrap();
        assert_ne!(styled, template);
        std::fs::write(
            project.join("templates/sw.js"),
            "/* worker implementation */",
        )
        .unwrap();
        assert_ne!(template, app_version("1.2.3", &layers).unwrap());
    }

    #[test]
    fn update_manifest_distinguishes_content_from_application_releases_without_pwa() {
        let dir = tempfile::tempdir().unwrap();
        let (mut config, mut sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let out = dir.path().join("out");
        let mut build_info = info(out.clone());
        let manifest = || -> serde_json::Value {
            serde_json::from_slice(&std::fs::read(out.join("updates.json")).unwrap()).unwrap()
        };
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let original = manifest();
        assert_eq!(
            original["entries"],
            serde_json::json!(["items/blog/2026-09-01-post-0/"])
        );

        build_info.now += Duration::minutes(1);
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        assert_eq!(manifest(), original, "rebuild time is not a release");

        config.site.build_max_bytes /= 2;
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let budgeted = manifest();
        assert_eq!(budgeted["app_version"], original["app_version"]);
        assert_ne!(budgeted["content_version"], original["content_version"]);

        config.site.title = "A new reader title".into();
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let titled = manifest();
        assert_eq!(titled["app_version"], original["app_version"]);
        assert_ne!(titled["content_version"], original["content_version"]);

        sources[0].name = Some("Updated publisher".into());
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let publisher = manifest();
        assert_eq!(publisher["app_version"], titled["app_version"]);
        assert_ne!(publisher["content_version"], titled["content_version"]);

        let front = crate::model::FrontMatter {
            title: "Newest article".into(),
            link: "https://blog.example/new".into(),
            source: "blog".into(),
            first_seen: day(19),
            published: Some(day(19)),
            ..Default::default()
        };
        store
            .write_item(crate::store::NewItem {
                dir: "items/blog/2026/09",
                stem: "2026-09-19-newest-article",
                front: &front,
                body: "New content",
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let article = manifest();
        assert_eq!(article["app_version"], publisher["app_version"]);
        assert_ne!(article["content_version"], publisher["content_version"]);
        assert_eq!(
            article["entries"][0],
            "items/blog/2026-09-19-newest-article/"
        );

        std::fs::create_dir(dir.path().join("static")).unwrap();
        std::fs::write(dir.path().join("static/style.css"), "body { color: teal; }").unwrap();
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let application = manifest();
        assert_ne!(application["app_version"], article["app_version"]);
        assert_eq!(application["content_version"], article["content_version"]);
    }

    #[test]
    fn item_pages_show_next_then_one_distinct_recommendation() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 6, "max_age_days = 30\npwa = false\n");
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();

        let newest =
            std::fs::read_to_string(out.join("items/blog/2026-09-06-post-5/index.html")).unwrap();
        assert!(
            newest.contains("data-next-url=\"items/blog/2026-09-05-post-4/\""),
            "{newest}"
        );
        assert!(!newest.contains("data-previous-url="), "{newest}");
        assert_eq!(newest.matches("class=\"article-more-link").count(), 2);
        assert_eq!(newest.matches("article-more-neighbor").count(), 0);
        assert!(!newest.contains("article-navigation-link"), "{newest}");
        assert!(!newest.contains("article-more-label"), "{newest}");

        let second =
            std::fs::read_to_string(out.join("items/blog/2026-09-05-post-4/index.html")).unwrap();
        assert!(second.contains("data-previous-url=\"items/blog/2026-09-06-post-5/\""));
        assert!(second.contains("data-next-url=\"items/blog/2026-09-04-post-3/\""));
        assert_eq!(second.matches("class=\"article-more-link").count(), 2);
        assert_eq!(second.matches("article-more-neighbor").count(), 0);
        assert!(!second.contains("article-navigation-link"), "{second}");

        let second_footer = second
            .split_once("<footer class=\"article-footer\"")
            .unwrap()
            .1;
        assert!(!second_footer.contains("rel=\"prev\""), "{second_footer}");
        let first_card_end = second_footer.find("</a>").unwrap();
        let first_card = &second_footer[..first_card_end];
        assert!(first_card.contains("rel=\"next\""), "{first_card}");
        assert!(first_card.contains(">Coming next</h2>"), "{first_card}");
        assert!(
            first_card.contains("items/blog/2026-09-04-post-3/"),
            "{first_card}"
        );
        let recommendation = &second_footer[first_card_end + "</a>".len()..];
        assert!(
            recommendation.contains("class=\"article-more-link title\""),
            "{recommendation}"
        );
        assert!(
            recommendation.contains(">Discover more</h2>"),
            "{recommendation}"
        );
        assert!(
            !recommendation.contains("items/blog/2026-09-04-post-3/"),
            "{recommendation}"
        );

        for day in 1..=6 {
            let page = std::fs::read_to_string(out.join(format!(
                "items/blog/2026-09-{day:02}-post-{}/index.html",
                day - 1
            )))
            .unwrap();
            let expected = if day == 1 { 1 } else { 2 };
            assert_eq!(page.matches("class=\"article-more-link").count(), expected);
            assert!(
                page.matches("class=\"article-more-link").count() <= 2,
                "day {day} exceeds the continuation-card limit"
            );
        }
    }

    #[test]
    fn continuation_omits_an_empty_footer_when_only_the_previous_article_exists() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 2, "max_age_days = 30\npwa = false\n");
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();

        let newest =
            std::fs::read_to_string(out.join("items/blog/2026-09-02-post-1/index.html")).unwrap();
        assert_eq!(newest.matches("class=\"article-more-link").count(), 1);
        assert!(newest.contains("rel=\"next\""), "{newest}");

        let oldest =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(oldest.contains("data-previous-url="), "{oldest}");
        assert!(
            !oldest.contains("<footer class=\"article-footer\""),
            "{oldest}"
        );
    }

    #[test]
    fn recommendations_reuse_feed_metadata_and_reserved_previews() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 2, "pwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 75)
            .encode_image(&image::DynamicImage::new_rgb8(160, 80))
            .unwrap();
        let hash = crate::model::sha1_hex(&bytes);
        item.front.preview = Some(crate::model::Preview {
            file: format!("{stem}.preview-{}.jpg", &hash[..12]),
            width: 160,
            height: 80,
            alt: Some("Preview".into()),
            color: None,
        });
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: Some(&bytes),
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let feed = scraper::Html::parse_document(
            &std::fs::read_to_string(out.join("index.html")).unwrap(),
        );
        let page = scraper::Html::parse_document(
            &std::fs::read_to_string(out.join("items/blog/2026-09-02-post-1/index.html")).unwrap(),
        );
        let row = feed
            .select(
                &scraper::Selector::parse(".row[data-url='items/blog/2026-09-01-post-0/']")
                    .unwrap(),
            )
            .next()
            .unwrap();
        let card = page
            .select(&scraper::Selector::parse(".article-more-card").unwrap())
            .next()
            .unwrap();
        let metadata = scraper::Selector::parse(".meta").unwrap();
        assert_eq!(
            site_relative_links(&row.select(&metadata).next().unwrap().inner_html()),
            site_relative_links(&card.select(&metadata).next().unwrap().inner_html())
        );
        let preview = scraper::Selector::parse(".preview-image").unwrap();
        let feed_image = row.select(&preview).next().unwrap();
        let card_image = card.select(&preview).next().unwrap();
        for attribute in ["src", "width", "height", "loading"] {
            assert_eq!(
                site_relative(feed_image.value().attr(attribute)),
                site_relative(card_image.value().attr(attribute))
            );
        }
    }

    #[test]
    fn article_resources_move_below_header_without_changing_tags_or_portable_links() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        item.front.labels = vec!["Interpretability".into()];
        item.front.summary =
            Some("HUGGING FACE MODELSCOPE TECHNICAL REPORT A publisher teaser.".into());
        item.body = "![Qwen-Scope main image](https://qianwen-res.oss-accelerate.aliyuncs.com/qwen-scope/Figures/overview.png)\n\n[HUGGING FACE](https://huggingface.co/collections/Qwen/qwen-scope) [MODELSCOPE & DATA](https://modelscope.cn/collections/Qwen/Qwen-Scope) [TECHNICAL REPORT](https://arxiv.org/abs/2605.11887?download=1&mode=full)\n\nInterpretability helps us understand how models work.\n".into();
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let article =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        let document = scraper::Html::parse_document(&article);
        let resources = document
            .select(&scraper::Selector::parse(".itemhead + .article-resources a").unwrap())
            .collect::<Vec<_>>();
        assert_eq!(resources.len(), 3);
        assert_eq!(
            resources[1].text().collect::<String>().trim(),
            "MODELSCOPE & DATA ↗"
        );
        let destinations = [
            "https://huggingface.co/collections/Qwen/qwen-scope",
            "https://modelscope.cn/collections/Qwen/Qwen-Scope",
            "https://arxiv.org/abs/2605.11887?download=1&mode=full",
        ];
        for (resource, destination) in resources.iter().zip(destinations) {
            assert_eq!(resource.value().attr("href"), Some(destination));
        }
        let body = document
            .select(&scraper::Selector::parse(".body").unwrap())
            .next()
            .unwrap();
        assert!(
            body.text()
                .collect::<String>()
                .trim()
                .starts_with("Interpretability")
        );
        assert!(!body.html().contains("TECHNICAL REPORT"));
        assert!(body.html().contains("qwen-scope/Figures/overview.png"));
        assert!(body.html().contains("Qwen-Scope main image"));
        let tags = document
            .select(&scraper::Selector::parse(".item-tags .tag").unwrap())
            .map(|tag| tag.text().collect::<String>())
            .collect::<Vec<_>>();
        assert_eq!(tags, ["#interpretability"]);
        assert!(article.contains("MODELSCOPE &amp; DATA"));
        let description = document
            .select(&scraper::Selector::parse("meta[name='description']").unwrap())
            .next()
            .unwrap()
            .value()
            .attr("content")
            .unwrap();
        assert!(description.contains("Interpretability helps"));
        assert!(!description.contains("HUGGING FACE"));
        for path in [
            "atom.xml",
            "rss.xml",
            "feed.json",
            "items/blog/2026-09-01-post-0.md",
            "items/blog/2026-09-01-post-0.txt",
            "items/blog/2026-09-01-post-0.rst",
            "items/blog/2026-09-01-post-0.json",
        ] {
            let output = std::fs::read_to_string(out.join(path)).unwrap();
            assert!(output.contains("HUGGING FACE"), "lost resources in {path}");
            assert!(
                output.contains("huggingface.co/collections/Qwen/qwen-scope"),
                "lost destination in {path}"
            );
            assert!(
                output.contains("arxiv.org/abs/2605.11887"),
                "lost report in {path}"
            );
        }
        assert!(
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0.txt"))
                .unwrap()
                .contains("TECHNICAL REPORT")
        );
        let retained = store.items().unwrap().remove(0);
        assert_eq!(retained.body, item.body);
        assert_eq!(retained.front.labels, item.front.labels);
        assert_eq!(retained.front.summary, item.front.summary);
    }

    #[test]
    fn published_titles_share_cleanup_across_all_representations() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        item.front.title = "🚀 Café #1 🧑🏽‍💻".into();
        item.body = "Body emoji stays 🚀.\n".into();
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        for path in [
            "index.html",
            "atom.xml",
            "rss.xml",
            "feed.json",
            "sources/blog.example/atom.xml",
            "sources/blog.example/rss.xml",
            "sources/blog.example/feed.json",
            "items/blog/2026-09-01-post-0/index.html",
            "items/blog/2026-09-01-post-0.md",
            "items/blog/2026-09-01-post-0.txt",
            "items/blog/2026-09-01-post-0.rst",
            "items/blog/2026-09-01-post-0.json",
        ] {
            let output = std::fs::read_to_string(out.join(path)).unwrap();
            assert!(output.contains("Café #1"), "missing title in {path}");
            assert!(!output.contains(&item.front.title), "raw title in {path}");
        }
        let markdown =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0.md")).unwrap();
        assert!(markdown.contains("Body emoji stays 🚀."));
        assert_eq!(store.items().unwrap()[0].front.title, item.front.title);
    }

    #[test]
    fn body_video_links_play_inline_in_the_reader_only() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        item.body = "Episode notes.\n\nhttps://youtu.be/abcDEF12345\n\nMore notes with https://youtu.be/inline1234 inline.\n".into();
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let page =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert_eq!(page.matches("video-player-inline").count(), 1, "{page}");
        assert!(
            page.contains("data-video-embed=\"https://www.youtube-nocookie.com/embed/abcDEF12345?"),
            "{page}"
        );
        // A link inside prose stays a link, and like every body link out of the site it opens
        // in a new tab without needing JavaScript.
        let inline_link = page
            .split_once("href=\"https://youtu.be/inline1234\"")
            .map(|(before, _)| before.rsplit("<a ").next().unwrap_or_default().to_string())
            .unwrap_or_default();
        assert!(inline_link.contains("target=\"_blank\""), "{page}");
        assert!(
            inline_link.contains("rel=\"external noopener noreferrer\""),
            "{page}"
        );
        assert!(
            page.contains(">https://youtu.be/inline1234</a> inline."),
            "{page}"
        );
        assert!(!page.contains("<iframe"), "{page}");
        for path in ["items/blog/2026-09-01-post-0.md", "feed.json", "atom.xml"] {
            let output = std::fs::read_to_string(out.join(path)).unwrap();
            assert!(
                !output.contains("video-player"),
                "facade leaked into {path}"
            );
            assert!(
                output.contains("youtu.be/abcDEF12345"),
                "link missing from {path}"
            );
        }
    }

    #[test]
    fn build_cleans_archived_social_threads_without_rewriting_sources() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        item.front.link = "https://mathstodon.xyz/@tao/117244102901892965".into();
        item.body = "First post preserves the fraction 1/3 and code `(1/3)`. (1/3)\n\n* * *\n\nSecond post. (2/3)\n\n* * *\n\nFinal post.\n\n3/3\n".into();
        let retained = "<section data-aggr-thread='activitypub'><article data-aggr-thread-post><p>First post preserves the fraction 1/3 and code <code>(1/3)</code>. (1/3)</p></article><hr><article data-aggr-thread-post><p>Second post. (2/3)</p></article><hr><article data-aggr-thread-post><p>Final post.</p><p>3/3</p></article></section>";
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        item.front.html = Some(format!("{stem}.html"));
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: &item.body,
                html: Some(retained),
                preview: None,
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        for path in [
            "items/blog/2026-09-01-post-0/index.html",
            "items/blog/2026-09-01-post-0.md",
            "items/blog/2026-09-01-post-0.json",
            "atom.xml",
            "rss.xml",
            "feed.json",
        ] {
            let output = std::fs::read_to_string(out.join(path)).unwrap();
            assert!(!output.contains("(2/3)"), "counter remains in {path}");
            assert!(!output.contains("3/3"), "counter remains in {path}");
            assert!(!output.contains("<hr"), "separator remains in {path}");
            assert!(!output.contains("* * *"), "separator remains in {path}");
            assert!(
                output.contains("fraction 1/3"),
                "fraction missing in {path}"
            );
        }
        let stored = store.items().unwrap().remove(0);
        assert_eq!(stored.body, item.body);
        assert_eq!(store.read_html(&stored).unwrap().as_deref(), Some(retained));
    }

    #[test]
    fn build_cleans_leading_publication_dates_from_existing_items() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "max_age_days = 30\npwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        item.body = "1st September 2026\n\nActual opening.\n".into();
        let stem = item.path.rsplit('/').next().unwrap();
        let dir_path = item.path.rsplit_once('/').unwrap().0;
        store
            .write_item(crate::store::NewItem {
                dir: dir_path,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();

        let html =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(!html.contains("1st September 2026"), "{html}");
        assert!(html.contains("Actual opening."), "{html}");
        let markdown =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0.md")).unwrap();
        assert!(!markdown.contains("1st September 2026"), "{markdown}");
    }

    #[test]
    fn build_cleans_existing_boundary_controls_without_rewriting_archive() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        item.body =
            "Advertisement\n\n•\n\nActual article.\n\n\\[[0 comments](https://publisher.example/#comment-form)\\]\n"
                .into();
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let html =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        let markdown =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0.md")).unwrap();
        assert!(!html.contains("0 comments"), "{html}");
        assert!(!markdown.contains("0 comments"), "{markdown}");
        assert!(html.contains("Actual article."), "{html}");
        for rendered in [&html, &markdown] {
            assert!(!rendered.contains("Advertisement"));
            assert!(!rendered.contains('•'));
        }
        for file in ["rss.xml", "atom.xml", "feed.json"] {
            let feed = std::fs::read_to_string(out.join(file)).unwrap();
            assert!(!feed.contains("Advertisement"), "{file}");
            assert!(!feed.contains('•'), "{file}");
            assert!(feed.contains("Actual article."), "{file}");
        }
        assert_eq!(store.items().unwrap()[0].body, item.body);
    }

    #[test]
    fn build_keeps_stored_markdown_authoritative_over_retained_html() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        let body = "```\n$ z dotfiles$ pwd/private/dotfiles\n```\n";
        let html = "<pre><code data-lang=\"bash\"><span><span>$ z dotfiles\n</span></span><span><span>$ <span>pwd</span>\n</span></span><span><span>/private/dotfiles\n</span></span></code></pre>";
        item.front.html = Some(format!("{stem}.html"));
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body,
                html: Some(html),
                preview: None,
                images: &[],
            })
            .unwrap();
        let stored = std::fs::read(dir.path().join("data").join(item.md_path())).unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("data").join(item.md_path())).unwrap(),
            stored
        );
        let markdown =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0.md")).unwrap();
        assert!(markdown.contains(body), "{markdown}");
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(out.join("items/blog/2026-09-01-post-0.json")).unwrap(),
        )
        .unwrap();
        assert!(
            json["content_markdown"]
                .as_str()
                .unwrap()
                .contains(body.trim())
        );
    }

    #[test]
    fn build_copies_valid_previews_into_optional_offline_assets() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = true\n");
        let mut item = store.items().unwrap().remove(0);
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 75)
            .encode_image(&image::DynamicImage::new_rgb8(160, 80))
            .unwrap();
        let hash = crate::model::sha1_hex(&bytes);
        item.front.preview = Some(crate::model::Preview {
            file: format!("{stem}.preview-{}.jpg", &hash[..12]),
            width: 160,
            height: 80,
            alt: Some("A local preview".into()),
            color: None,
        });
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: Some(&bytes),
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        let mut build_info = info(out.clone());
        build_info.release = true;
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let path = format!("assets/previews/{hash}.jpg");
        assert_eq!(std::fs::read(out.join(&path)).unwrap(), bytes);
        assert!(
            !assets::precache_entries(&out, vec![path.clone()], &BTreeMap::new()).unwrap()[0]
                .required
        );
        // Media is content-addressed and cached on first view, not listed at install time.
        let worker = std::fs::read_to_string(out.join("sw.js")).unwrap();
        assert!(!worker.contains(&path));
        assert_eq!(
            hash,
            crate::model::sha1_hex(std::fs::read(out.join(&path)).unwrap())
        );
        let page =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(page.contains(&format!("https://u.github.io/repo/{path}")));
        let feed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("feed.json")).unwrap()).unwrap();
        assert_eq!(
            feed["items"][0]["image"],
            format!("https://u.github.io/repo/{path}")
        );
    }

    #[test]
    fn youtube_article_poster_keeps_feed_thumbnail_small_and_local() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "");
        let mut item = store.items().unwrap().remove(0);
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        item.front.link = "https://www.youtube.com/watch?v=video123".into();
        item.body = "Video description.\n".into();
        let pixels = image::RgbaImage::from_pixel(1280, 720, image::Rgba([21, 82, 109, 255]));
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(pixels)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let asset = crate::media::prepare_asset(
            &crate::sources::youtube::poster_candidates(
                &url::Url::parse(&item.front.link).unwrap(),
            )[0],
            encoded.into_inner(),
            &crate::media::MediaLimits::default(),
        )
        .unwrap();
        let preview = crate::preview::thumbnail(&asset.master_bytes, None).unwrap();
        item.front.images = vec![asset.metadata(stem)];
        item.front.preview = Some(preview.metadata(stem));
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: Some(&preview.bytes),
                images: std::slice::from_ref(&asset),
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let page =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(page.contains("video-preview"), "{page}");
        assert!(page.contains("width=\"1280\" height=\"720\""), "{page}");
        assert!(page.contains("640w"), "{page}");
        assert!(page.contains("1280w"), "{page}");
        assert!(!page.contains("src=\"https://i.ytimg.com"), "{page}");
        let feed = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(!feed.contains("assets/images/"), "{feed}");
        assert!(feed.contains("width=\"256\" height=\"144\""), "{feed}");
        let export: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("feed.json")).unwrap()).unwrap();
        assert!(
            export["items"][0]["image"]
                .as_str()
                .unwrap()
                .contains("/assets/previews/")
        );
        assert_eq!(
            store
                .read_preview(&store.items().unwrap()[0])
                .unwrap()
                .unwrap(),
            preview.bytes
        );
    }

    #[test]
    fn build_publishes_lossless_article_images_without_changing_portable_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = true\n");
        let mut item = store.items().unwrap().remove(0);
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        let source = "https://blog.example/article-diagram.png";
        item.body = format!("Before.\n\n![Article diagram]({source})\n\nAfter.\n");
        let pixels = image::RgbaImage::from_pixel(640, 320, image::Rgba([21, 82, 109, 255]));
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(pixels)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let asset = crate::media::prepare_asset(
            &crate::media::Candidate {
                url: url::Url::parse(source).unwrap(),
                alt: Some("Article diagram".into()),
            },
            encoded.into_inner(),
            &crate::media::MediaLimits::default(),
        )
        .unwrap();
        item.front.images = vec![asset.metadata(stem)];
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: None,
                images: std::slice::from_ref(&asset),
            })
            .unwrap();

        let out = dir.path().join("out");
        let mut build_info = info(out.clone());
        build_info.release = true;
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();

        let feed = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(
            feed.contains("assets/previews/"),
            "archived media supplies missing preview: {feed}"
        );
        assert!(store.items().unwrap()[0].front.preview.is_none());

        let master = format!("assets/images/{}.png", asset.master_hash);
        assert_eq!(
            std::fs::read(out.join(&master)).unwrap(),
            asset.master_bytes
        );
        let page =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(
            page.contains("<picture class=\"article-picture\""),
            "{page}"
        );
        assert!(
            page.contains(&format!("src=\"../../../{master}\"")),
            "{page}"
        );
        assert!(page.contains("type=\"image/webp\""), "{page}");
        assert!(page.contains("width=\"640\" height=\"320\""), "{page}");
        assert!(page.contains("fetchpriority=\"high\""), "{page}");
        for rendition in &asset.renditions {
            let path = format!("assets/images/{}.webp", rendition.hash);
            assert_eq!(std::fs::read(out.join(&path)).unwrap(), rendition.bytes);
            assert!(page.contains(&path), "{page}");
        }

        let markdown =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0.md")).unwrap();
        assert!(markdown.contains(source), "{markdown}");
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(out.join("items/blog/2026-09-01-post-0.json")).unwrap(),
        )
        .unwrap();
        assert!(json["content_html"].as_str().unwrap().contains(source));
        assert!(!json["content_html"].as_str().unwrap().contains(&master));

        // Every rendition is published under its own content hash and cached when first shown.
        let published = published_media(&out);
        assert!(published.contains_key(&master), "{published:?}");
        for rendition in &asset.renditions {
            assert!(
                published.contains_key(&format!("assets/images/{}.webp", rendition.hash)),
                "{published:?}"
            );
        }
        let revisions = published_media(&out);
        for path in std::iter::once(master.clone()).chain(
            asset
                .renditions
                .iter()
                .map(|rendition| format!("assets/images/{}.webp", rendition.hash)),
        ) {
            assert_eq!(
                revisions[&path],
                crate::model::sha1_hex(std::fs::read(out.join(&path)).unwrap()),
                "{path} is revisioned by its own content hash"
            );
        }
    }

    #[test]
    fn middle_pages_link_to_adjacent_and_edge_pages() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 5, "items_per_page = 1\npwa = false\n");
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();

        let middle = std::fs::read_to_string(out.join("page/3/index.html")).unwrap();
        assert!(middle.contains("href=\"../../\">first</a>"), "{middle}");
        assert!(
            middle.contains("href=\"../../page/2/\">newer</a>"),
            "{middle}"
        );
        assert!(
            middle.contains("href=\"../../page/4/\">older</a>"),
            "{middle}"
        );
        assert!(
            middle.contains("href=\"../../page/5/\">last</a>"),
            "{middle}"
        );
        assert!(middle.contains("data-static-first=\"\""), "{middle}");
        assert!(middle.contains("data-static-next=\"page/4/\""), "{middle}");
        assert!(!middle.contains("<base "), "{middle}");
        assert!(
            middle.contains("<link rel=\"canonical\" href=\"https://u.github.io/repo/page/3/\">"),
            "{middle}"
        );
        assert!(
            middle.contains("<link rel=\"first\" href=\"../../\">"),
            "{middle}"
        );
        assert!(
            middle.contains("<link rel=\"prev\" href=\"../../page/2/\">"),
            "{middle}"
        );
        assert!(
            middle.contains("<link rel=\"next\" href=\"../../page/4/\">"),
            "{middle}"
        );
        assert!(
            middle.contains("<link rel=\"last\" href=\"../../page/5/\">"),
            "{middle}"
        );

        let second = std::fs::read_to_string(out.join("page/2/index.html")).unwrap();
        assert!(second.contains("href=\"../../\">first</a>"), "{second}");
        assert!(second.contains("href=\"../../\">newer</a>"), "{second}");
        let first = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(first.contains("href=\"./page/2/\">older</a>"), "{first}");
    }

    #[test]
    fn pwa_outputs_cover_the_shells_and_the_newest_items() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(
            dir.path(),
            3,
            "preferences.offline_items = 2\nindexing = true\n",
        );
        let out = dir.path().join("out");
        let mut build_info = info(out.clone());
        build_info.release = true;
        let summary = build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        assert_eq!(summary.items, 3);
        // River, combined publisher/feed archive, six utility/directory pages, and the offline page.
        assert_eq!(summary.pages, 1 + 1 + 6 + 1);
        assert!(!out.join("categories/blog").exists());
        let home = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(home.contains("href=\"./browse/\""), "{home}");
        assert!(home.contains(">browse</"), "{home}");
        let document = scraper::Html::parse_document(&home);
        let primary = scraper::Selector::parse(".nav-primary a").unwrap();
        assert_eq!(
            document
                .select(&primary)
                .filter_map(|link| link.value().attr("href"))
                .collect::<Vec<_>>(),
            ["./", "./browse/", "./preferences/"]
        );
        let actions = scraper::Selector::parse(".nav-actions a").unwrap();
        assert_eq!(document.select(&actions).count(), 1);
        assert!(!home.contains("<dd>Categories</dd>"), "{home}");
        let browse = std::fs::read_to_string(out.join("browse/index.html")).unwrap();
        assert!(browse.contains("id=\"sources\""), "{browse}");
        assert!(browse.contains("id=\"tags\""), "{browse}");
        assert!(browse.contains("id=\"categories\""), "{browse}");
        assert!(browse.contains("0 categories."), "{browse}");
        assert!(browse.contains("0 tags."), "{browse}");
        assert!(browse.contains("in <code>aggr.toml</code>"), "{browse}");
        assert!(
            browse.contains("href=\"../?q=source%3A%22blog.example%22\""),
            "{browse}"
        );
        assert!(!browse.contains("explore-nav"), "{browse}");
        assert!(!browse.contains("directory-count"), "{browse}");
        assert!(!browse.contains("directory-status"), "{browse}");
        for removed in ["explore", "library", "search"] {
            assert!(!out.join(removed).exists());
        }

        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("manifest.webmanifest")).unwrap())
                .unwrap();
        assert_eq!(manifest["name"], "Demo <site>");
        assert_eq!(manifest["short_name"], "aggr");
        assert_eq!(
            manifest["description"],
            "Browse Demo <site>, an independent, searchable archive of readable snapshots from followed feeds, preserved in Git with aggr."
        );
        assert_eq!(manifest["start_url"], "./");
        assert_eq!(manifest["scope"], "./");
        let browse_shortcut = manifest["shortcuts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|shortcut| shortcut["name"] == "Browse")
            .unwrap();
        assert_eq!(browse_shortcut["url"], "./browse/");
        assert_eq!(manifest["display"], "standalone");
        assert_eq!(
            manifest["display_override"],
            serde_json::json!(["standalone", "minimal-ui"])
        );
        assert_eq!(manifest["background_color"], "#f5f6fa");
        assert_eq!(manifest["icons"].as_array().unwrap().len(), 4);
        assert!(
            manifest["icons"][0]["src"]
                .as_str()
                .unwrap()
                .starts_with("./assets/icon-192-")
        );
        for icon in [
            "icon-192-",
            "icon-512-",
            "icon-maskable-512-",
            "apple-touch-icon-",
        ] {
            let path = std::fs::read_dir(out.join("assets"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(icon)
                })
                .unwrap();
            let png = std::fs::read(path).unwrap();
            assert!(png.starts_with(b"\x89PNG"), "{icon} is not a PNG");
            assert_eq!(png.get(25), Some(&2), "{icon} must be an opaque RGB PNG");
        }

        let sw = std::fs::read_to_string(out.join("sw.js")).unwrap();
        let version = sw
            .split("var VERSION = \"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert_eq!(version.len(), 40);
        assert!(version.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(sw.contains("new URL(\"./\", self.registration.scope)"));
        assert!(sw.contains("\"assets/style-"));
        assert!(!sw.contains("\"pagefind/pagefind.js\""));
        assert!(!out.join("search-meta.json").exists());
        assert!(
            !sw.contains("assets/logo-"),
            "the multi-megabyte source icon is not precached"
        );
        assert!(sw.contains("navigationPreload.enable"));
        // Pages are network-first with a cached fallback; content-addressed assets are not.
        assert!(sw.contains("function pageResponse(request)"));
        assert!(sw.contains("function assetResponse(request, name, limit)"));
        assert!(sw.contains("var OFFLINE = SCOPE + \"offline.html\""));
        // Nothing is downloaded ahead of the reader any more.
        assert!(!sw.contains("OFFLINE_CATALOG"));
        assert!(!sw.contains("OFFLINE_COUNT"));
        assert!(!sw.contains("search-manifest.json"));
        assert!(sw.contains("\"offline.html\""));
        assert!(sw.contains("\"browse/\""));
        // Article pages are cached when opened, not listed at install time.
        assert!(!sw.contains("\"items/"), "{sw}");

        let offline = std::fs::read_to_string(out.join("offline.html")).unwrap();
        assert!(!offline.contains("id=\"offline-articles\""));
        assert!(offline.contains("<link rel=\"manifest\" href=\"/repo/manifest.webmanifest\">"));
        assert!(offline.contains("pwa: true"));
        assert!(offline.contains("href=\"/repo/browse/\""), "{offline}");
        assert!(offline.contains("base: \"/repo/\""), "{offline}");
        assert!(!offline.contains("<base "), "{offline}");
        assert!(!offline.contains("rel=\"canonical\""));
        assert!(!offline.contains("application/ld+json"));
        let not_found = std::fs::read_to_string(out.join("404.html")).unwrap();
        assert!(not_found.contains("href=\"/repo/browse/\""), "{not_found}");
        assert!(not_found.contains("base: \"/repo/\""), "{not_found}");
        assert!(!not_found.contains("<base "), "{not_found}");
        assert!(!not_found.contains("rel=\"canonical\""));
        assert!(!not_found.contains("property=\"og:url\""));
        assert!(!not_found.contains("application/ld+json"));
        let river = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(
            river.contains("<title>Demo &lt;site&gt; | aggr</title>"),
            "{river}"
        );
        assert!(
            river.contains("<meta name=\"application-name\" content=\"Demo &lt;site&gt;\">"),
            "{river}"
        );
        assert!(
            river.contains(
                "<meta name=\"apple-mobile-web-app-title\" content=\"Demo &lt;site&gt;\">"
            ),
            "{river}"
        );
        assert!(
            river.contains(
                "<meta name=\"description\" content=\"Browse Demo &lt;site&gt;, an independent, searchable archive of readable snapshots from followed feeds, preserved in Git with aggr.\">"
            ),
            "{river}"
        );
        assert!(river.contains("type=\"application/ld+json\""), "{river}");
        assert!(!river.contains("#identity"), "{river}");
        assert!(river.contains("https://schema.org"), "{river}");
        assert!(river.contains("rel=\"search\""), "{river}");
        assert!(river.contains("rel=\"service-meta\""), "{river}");
        assert!(river.contains("rel=\"linkset\""), "{river}");
        assert!(river.contains("rel=\"type\""), "{river}");
        assert!(river.contains("name=\"aggr:network\""), "{river}");
        assert!(
            river.contains("name=\"robots\" content=\"index,follow,"),
            "{river}"
        );
        assert!(river.contains("rel=\"apple-touch-icon\""));
        assert!(river.contains("name=\"theme-color\""));
        assert!(river.contains("href=\"./browse/\""), "{river}");
        assert!(river.contains("href=\"./preferences/\""), "{river}");
        assert!(
            river.contains("aggr.toml <span aria-hidden=\"true\">↗</span></a>"),
            "{river}"
        );
        assert!(out.join("preferences/index.html").is_file());
        assert!(!out.join("settings/index.html").exists());
        assert!(!out.join("pagefind/pagefind.js").exists());
        for path in ["preferences/index.html", "offline.html", "404.html"] {
            let utility = std::fs::read_to_string(out.join(path)).unwrap();
            assert!(
                utility.contains("name=\"robots\" content=\"noindex,follow\""),
                "{path}: {utility}"
            );
        }
        let descriptor: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("aggr.json")).unwrap()).unwrap();
        assert_eq!(descriptor["type"], "aggr-instance");
        assert_eq!(
            descriptor["network"],
            "https://github.com/aymericbeaumet/aggr#network"
        );
        assert_eq!(descriptor["url"], "https://u.github.io/repo/");
        assert!(
            descriptor["source"]["config"]
                .as_str()
                .is_some_and(|url| url.ends_with("/aggr.toml")),
            "{descriptor}"
        );
        assert_eq!(
            descriptor["feeds"]["json"],
            "https://u.github.io/repo/feed.json"
        );
        assert_eq!(
            descriptor["discovery"]["linkset"],
            "https://u.github.io/repo/linkset.json"
        );
        let linkset: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("linkset.json")).unwrap()).unwrap();
        let original = linkset["linkset"]
            .as_array()
            .unwrap()
            .iter()
            .find(|context| context["anchor"] == "https://blog.example/2")
            .unwrap();
        assert_eq!(
            original["https://schema.org/archivedAt"][0]["href"],
            "https://u.github.io/repo/items/blog/2026-09-03-post-2/"
        );
        assert!(out.join("sources/blog.example/atom.xml").is_file());
        assert!(out.join("sources/blog.example/rss.xml").is_file());
        assert!(out.join("sources/blog.example/feed.json").is_file());
        let atom = std::fs::read_to_string(out.join("sources/blog.example/atom.xml")).unwrap();
        assert!(atom.contains("xml:lang=\"en\""), "{atom}");
        assert!(atom.contains("<id>urn:aggr:item:"), "{atom}");
        assert!(atom.contains("<link rel=\"via\""), "{atom}");
        assert!(atom.contains("<content type=\"html\">"), "{atom}");
        let opensearch = std::fs::read_to_string(out.join("opensearch.xml")).unwrap();
        assert!(
            opensearch.contains("https://u.github.io/repo/?q={searchTerms}"),
            "{opensearch}"
        );
        let sitemap = std::fs::read_to_string(out.join("sitemap.xml")).unwrap();
        assert!(
            sitemap.contains("https://u.github.io/repo/browse/"),
            "{sitemap}"
        );
        assert!(
            sitemap.contains("<loc>https://u.github.io/repo/sources/</loc>"),
            "{sitemap}"
        );
        assert!(
            sitemap.contains("https://u.github.io/repo/items/blog/2026-09-03-post-2/"),
            "{sitemap}"
        );
        assert!(
            !out.join("robots.txt").exists(),
            "a robots file under a project subpath cannot govern the origin"
        );
        let item =
            std::fs::read_to_string(out.join("items/blog/2026-09-03-post-2/index.html")).unwrap();
        assert!(
            item.contains("<link rel=\"canonical\" href=\"https://u.github.io/repo/items/blog/2026-09-03-post-2/\">"),
            "{item}"
        );
        assert!(
            item.contains("<link rel=\"via\" href=\"https://blog.example/2\">"),
            "{item}"
        );
        assert!(
            item.contains("<link rel=\"original\" href=\"https://blog.example/2\">"),
            "{item}"
        );
        assert!(!item.contains("class=\"archive-note\""), "{item}");
        assert!(!item.contains("Git record <code>"), "{item}");
        assert!(item.contains("class=\"article-more\""), "{item}");
        assert!(item.contains("isBasedOn"), "{item}");
        assert!(item.contains("ArchiveComponent"), "{item}");
        assert!(item.contains("archivedAt"), "{item}");
        assert!(!item.contains("BlogPosting"), "{item}");
        let schema = item
            .split_once("<script type=\"application/ld+json\">")
            .and_then(|(_, rest)| rest.split_once("</script>"))
            .map(|(json, _)| serde_json::from_str::<serde_json::Value>(json).unwrap())
            .unwrap();
        let html_snapshot = schema["@graph"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| {
                node["@type"]
                    .as_array()
                    .is_some_and(|types| types.iter().any(|kind| kind == "ArchiveComponent"))
            })
            .unwrap();
        let html_original = schema["@graph"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["@id"] == "https://blog.example/2")
            .unwrap();
        assert!(
            html_original["wordCount"]
                .as_u64()
                .is_some_and(|count| count > 0)
        );
        assert!(
            html_original["timeRequired"]
                .as_str()
                .is_some_and(|duration| duration.starts_with("PT") && duration.ends_with('M'))
        );
        let json_snapshot: serde_json::Value = serde_json::from_slice(
            &std::fs::read(out.join("items/blog/2026-09-03-post-2.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(html_snapshot["@id"], json_snapshot["@id"]);
        assert_eq!(html_original["wordCount"], json_snapshot["word_count"]);
        assert_eq!(
            html_original["timeRequired"],
            json_snapshot["mainEntity"]["timeRequired"]
        );
        assert_eq!(
            json_snapshot["@id"],
            "https://u.github.io/repo/items/blog/2026-09-03-post-2/#webpage"
        );
    }

    #[test]
    fn article_tags_publish_from_old_bodies_without_mutating_the_archive() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        item.front.labels = vec!["AI".into(), "hand picked".into()];
        item.body =
            "Video description mentions #interior in prose.\n\n#Jev #ai #programming #coding\n"
                .into();
        store
            .write_item(crate::store::NewItem {
                dir: "items/blog/2026/09",
                stem: "2026-09-01-post-0",
                front: &item.front,
                body: &item.body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let markdown =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0.md")).unwrap();
        let (front, body) =
            crate::store::frontmatter::parse::<crate::model::FrontMatter>(&markdown).unwrap();
        assert_eq!(
            front.labels,
            ["ai", "coding", "hand picked", "jev", "programming"]
        );
        assert_eq!(body, "Video description mentions #interior in prose.\n");
        let feed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("feed.json")).unwrap()).unwrap();
        assert_eq!(
            feed["items"][0]["tags"],
            serde_json::json!(["ai", "coding", "hand picked", "jev", "programming"])
        );
        let search: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("search-catalog.json")).unwrap())
                .unwrap();
        for tag in ["ai", "coding", "hand-picked", "jev", "programming"] {
            assert!(out.join(format!("tags/{tag}/index.html")).is_file());
            assert!(
                search["facets"]["tag"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|facet| facet["value"] == tag && facet["count"] == 1)
            );
        }
        assert_eq!(store.read_item(&item.path).unwrap(), item);
    }

    #[test]
    fn rendered_titles_preserve_article_and_source_case_with_lowercase_topics() {
        let dir = tempfile::tempdir().unwrap();
        let (config, mut sources, store) = fixture(dir.path(), 1, "");
        sources[0].name = Some("The Example Blog".into());
        sources[0].category = Some("Computer SCIENCE".into());
        let mut item = store.items().unwrap().remove(0);
        item.front.labels = vec!["#Rust".into(), "RUST".into(), "Generative AI".into()];
        store
            .write_item(crate::store::NewItem {
                dir: "items/blog/2026/09",
                stem: "2026-09-01-post-0",
                front: &item.front,
                body: &item.body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        for (path, title) in [
            ("items/blog/2026-09-01-post-0/index.html", "Post 0"),
            ("sources/blog.example/index.html", "The Example Blog"),
            ("categories/computer-science/index.html", "computer science"),
            ("tags/rust/index.html", "rust"),
            ("tags/generative-ai/index.html", "generative ai"),
            ("browse/index.html", "browse"),
        ] {
            let html = std::fs::read_to_string(out.join(path)).unwrap();
            assert!(
                html.contains(&format!(
                    "<title>{title} | Demo &lt;site&gt; | aggr</title>"
                )),
                "{path}: {html}"
            );
            assert!(!html.contains("Computer SCIENCE"), "{path}");
            assert!(!html.contains("Generative AI"), "{path}");
            assert!(!html.contains(">#Rust<"), "{path}");
        }
        let article =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(article.contains("content=\"computer science\""));
        assert!(article.contains(">#rust</a>"));
        assert!(article.contains(">#generative ai</a>"));
    }

    #[test]
    fn seo_metadata_is_configurable_unique_and_identifies_the_publisher() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(
            dir.path(),
            2,
            r#"description = "Independent reading notes and useful links."
items_per_page = 1
repository = "owner/reader"

[site.identity]
type = "person"
name = "Ada Example"
url = "https://example.com/ada"
same_as = ["https://social.example/@ada"]
"#,
        );
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();

        let home = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(
            home.contains("<title>Demo &lt;site&gt; | aggr</title>"),
            "{home}"
        );
        assert!(
            home.contains(
                "<meta name=\"description\" content=\"Independent reading notes and useful links.\">"
            ),
            "{home}"
        );

        let second = std::fs::read_to_string(out.join("page/2/index.html")).unwrap();
        assert!(
            second.contains("<title>Demo &lt;site&gt; — page 2 | aggr</title>"),
            "{second}"
        );
        assert!(
            second.contains(
                "<meta name=\"description\" content=\"Independent reading notes and useful links Page 2.\">"
            ),
            "{second}"
        );
        let source = std::fs::read_to_string(out.join("sources/blog.example/index.html")).unwrap();
        assert!(
            source.contains(
                "<meta name=\"description\" content=\"Browse retained readable snapshots from blog.example in Demo &lt;site&gt;, with original URLs and capture dates.\">"
            ),
            "{source}"
        );
        let item =
            std::fs::read_to_string(out.join("items/blog/2026-09-02-post-1/index.html")).unwrap();
        assert!(
            item.contains("<title>Post 1 | Demo &lt;site&gt; | aggr</title>"),
            "{item}"
        );
        assert!(
            item.contains("Archived readable snapshot of Post 1 from blog.example, first captured 2026-09-02 and preserved by Demo &lt;site&gt;."),
            "{item}"
        );

        let schema_start = "<script type=\"application/ld+json\">";
        let schema = home
            .split_once(schema_start)
            .and_then(|(_, rest)| rest.split_once("</script>"))
            .map(|(json, _)| serde_json::from_str::<serde_json::Value>(json).unwrap())
            .unwrap();
        let graph = schema["@graph"].as_array().unwrap();
        let identity = graph.iter().find(|node| node["@type"] == "Person").unwrap();
        assert_eq!(identity["name"], "Ada Example");
        assert_eq!(identity["url"], "https://example.com/ada");
        assert_eq!(identity["sameAs"][0], "https://social.example/@ada");
        let website = graph
            .iter()
            .find(|node| node["@type"] == "WebSite")
            .unwrap();
        assert_eq!(website["creator"]["@id"], identity["@id"]);
        assert_eq!(website["publisher"]["@id"], identity["@id"]);

        let llms = std::fs::read_to_string(out.join("llms.txt")).unwrap();
        assert!(llms.starts_with("# Demo <site>\n\n> Independent reading notes"));
        assert!(llms.contains("[Instance metadata](https://u.github.io/repo/aggr.json)"));
        assert!(!llms.contains("[Sitemap]"));
        assert!(
            llms.contains(
                "[Original URL to snapshot linkset](https://u.github.io/repo/linkset.json)"
            )
        );
        assert!(llms.contains("[Source configuration](https://raw.githubusercontent.com/"));
        assert!(!llms.contains("[Categories]"), "{llms}");
    }

    #[test]
    fn sitemap_uses_local_capture_time_for_a_newly_archived_old_article() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\nindexing = true\n");
        let mut item = store.items().unwrap().remove(0);
        item.front.replicated_at = Some(day(15));
        let stem = item.path.rsplit('/').next().unwrap();
        let item_dir = item.path.rsplit_once('/').unwrap().0;
        store
            .write_item(crate::store::NewItem {
                dir: item_dir,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();

        let out = dir.path().join("out");
        let mut build_info = info(out.clone());
        build_info.release = true;
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let sitemap = std::fs::read_to_string(out.join("sitemap.xml")).unwrap();
        let entry = sitemap
            .split("<url>")
            .find(|entry| entry.contains("items/blog/2026-09-01-post-0/"))
            .unwrap();
        assert!(
            entry.contains("<lastmod>2026-09-15T00:00:00Z</lastmod>"),
            "{entry}"
        );
        let page =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(page.contains("first captured"), "{page}");
        assert!(
            page.contains("\"dateCreated\":\"2026-09-15T00:00:00+00:00\""),
            "{page}"
        );
        assert!(!page.contains("replicated here"), "{page}");
        assert!(!page.contains("class=\"archive-note\""), "{page}");
    }

    #[test]
    fn config_navigation_uses_github_page_while_metadata_keeps_raw_url() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(
            dir.path(),
            1,
            "repository = \"owner/reader\"\npwa = false\n",
        );
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();

        let home = std::fs::read_to_string(out.join("index.html")).unwrap();
        let sha = "c".repeat(40);
        assert!(
            home.contains(&format!(
                "href=\"https://github.com/owner/reader/blob/{sha}/aggr.toml\""
            )),
            "{home}"
        );
        assert!(
            home.contains(&format!(
                "name=\"aggr:source\" content=\"https://raw.githubusercontent.com/owner/reader/{sha}/aggr.toml\""
            )),
            "{home}"
        );
    }

    #[test]
    fn indexing_policy_preserves_reader_search_and_discovery() {
        for (extra, release, development, indexable) in [
            ("", true, false, false),
            ("indexing = true\n", true, false, true),
            ("indexing = true\n", false, false, false),
            ("indexing = true\n", true, true, false),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (config, sources, store) = fixture(dir.path(), 1, extra);
            let out = dir.path().join("out");
            let mut build_info = info(out.clone());
            build_info.base_url = Some("https://reads.example/".into());
            build_info.release = release;
            build_info.development = development;
            build(&config, &sources, &store, dir.path(), &build_info).unwrap();

            for path in [
                "index.html",
                "browse/index.html",
                "sources/index.html",
                "sources/blog.example/index.html",
                "items/blog/2026-09-01-post-0/index.html",
                "preferences/index.html",
                "404.html",
                "offline.html",
            ] {
                let html = std::fs::read_to_string(out.join(path)).unwrap();
                let utility =
                    matches!(path, "preferences/index.html" | "404.html" | "offline.html");
                let directive = if indexable && !utility {
                    "name=\"robots\" content=\"index,follow,"
                } else {
                    "name=\"robots\" content=\"noindex,follow\""
                };
                assert!(
                    html.contains(directive),
                    "{extra:?}, release={release}, {path}"
                );
            }
            assert_eq!(out.join("sitemap.xml").exists(), indexable);
            let robots = std::fs::read_to_string(out.join("robots.txt")).unwrap();
            assert!(robots.contains("Allow: /"));
            assert_eq!(robots.contains("Sitemap:"), indexable);
            let descriptor: serde_json::Value =
                serde_json::from_slice(&std::fs::read(out.join("aggr.json")).unwrap()).unwrap();
            assert_eq!(descriptor["discovery"].get("sitemap").is_some(), indexable);
            let inventory = std::fs::read_to_string(out.join("llms.txt")).unwrap();
            assert_eq!(inventory.contains("[Sitemap]"), indexable);
            for path in [
                "search-catalog.json",
                "opensearch.xml",
                "linkset.json",
                "atom.xml",
                "sources.opml",
            ] {
                assert!(out.join(path).is_file(), "{path}");
            }
            let search: serde_json::Value =
                serde_json::from_slice(&std::fs::read(out.join("search-catalog.json")).unwrap())
                    .unwrap();
            assert_eq!(search["docs"], 1, "local search retains the article");
        }
    }

    #[test]
    fn origin_root_build_advertises_its_sitemap_in_robots() {
        let dir = tempfile::tempdir().unwrap();
        let (mut config, sources, store) = fixture(dir.path(), 1, "pwa = false\nindexing = true\n");
        let out = dir.path().join("out");
        let mut build_info = info(out.clone());
        build_info.base_url = Some("https://reads.example/".into());
        build_info.release = true;
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();

        let robots = std::fs::read_to_string(out.join("robots.txt")).unwrap();
        assert_eq!(
            robots,
            "User-agent: *\nAllow: /\nSitemap: https://reads.example/sitemap.xml\n"
        );

        config.site.indexing = false;
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        assert!(
            !out.join("sitemap.xml").exists(),
            "stale sitemap must be removed"
        );
        assert_eq!(
            std::fs::read_to_string(out.join("robots.txt")).unwrap(),
            "User-agent: *\nAllow: /\n"
        );
    }

    #[test]
    fn pwa_off_writes_no_manifest_worker_or_offline_page() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let out = dir.path().join("out");
        let summary = build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        assert_eq!(summary.pages, 1 + 1 + 6);
        for name in ["manifest.webmanifest", "sw.js", "offline.html"] {
            assert!(!out.join(name).exists(), "{name} was written");
        }
        let river = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(!river.contains("rel=\"manifest\""));
        assert!(!river.contains("mobile-web-app-capable"));
        assert!(!river.contains("apple-mobile-web-app-capable"));
        assert!(river.contains("pwa: false"));
    }

    #[test]
    fn interactive_originals_keep_fallbacks_and_document_alternates_keep_original_identity() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 2, "pwa = false\n");
        let mut items = store.items().unwrap();
        items.sort_by(|left, right| left.path.cmp(&right.path));
        items[0]
            .front
            .extra
            .insert("content:interactive".into(), true.into());
        items[1].front.extra.insert(
            "document_url".into(),
            "https://papers.test/verified.pdf".into(),
        );
        for item in &items {
            let (directory, stem) = item.path.rsplit_once('/').unwrap();
            store
                .write_item(crate::store::NewItem {
                    dir: directory,
                    stem,
                    front: &item.front,
                    body: &item.body,
                    html: None,
                    preview: None,
                    images: &[],
                })
                .unwrap();
        }
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let interactive =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(
            interactive
                .contains("<iframe class=\"interactive-viewer\" src=\"https://blog.example/0\""),
            "{interactive}"
        );
        assert!(
            interactive.contains("sandbox=\"allow-scripts\" referrerpolicy=\"no-referrer\""),
            "{interactive}"
        );
        assert!(
            interactive
                .contains(">Open original <span aria-hidden=\"true\">↗</span></a></figcaption>"),
            "{interactive}"
        );
        assert!(!interactive.contains("if the interactive view is unavailable"));
        let document =
            std::fs::read_to_string(out.join("items/blog/2026-09-02-post-1/index.html")).unwrap();
        assert!(document.contains("data=\"https://papers.test/verified.pdf\""));
        assert!(document.contains("href=\"https://blog.example/1\""));
        let feed = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(!feed.contains("data-interactive-embed"));
        assert!(!feed.contains("<embed"));
    }

    #[test]
    fn stored_documents_publish_same_origin_with_budget_and_offline_resources() {
        let dir = tempfile::tempdir().unwrap();
        let (mut config, sources, store) = fixture(dir.path(), 1, "");
        let mut item = store.items().unwrap().remove(0);
        item.front.link = "http://publisher.invalid/paper.pdf#page=2".into();
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        let mut bytes = vec![b' '; 512 * 1024];
        bytes[..8].copy_from_slice(b"%PDF-1.7");
        let asset = crate::document::Asset {
            source_url: "http://publisher.invalid/paper.pdf".into(),
            bytes,
        };
        store
            .write_item_with_document(
                crate::store::NewItem {
                    dir: directory,
                    stem,
                    front: &item.front,
                    body: "Abstract.",
                    html: None,
                    preview: None,
                    images: &[],
                },
                Some(&asset),
            )
            .unwrap();
        let build_info = info(dir.path().join("out"));
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let hash = crate::model::sha1_hex(&asset.bytes);
        let local = format!("assets/documents/{hash}.pdf");
        let page_path = "items/blog/2026-09-01-post-0/index.html";
        let page = std::fs::read_to_string(build_info.out.join(page_path)).unwrap();
        assert!(
            page.contains(&format!("data=\"../../../{local}#page=2\"")),
            "{page}"
        );
        assert!(
            page.contains("href=\"http://publisher.invalid/paper.pdf#page=2\""),
            "original provenance remains visible"
        );
        assert_eq!(
            std::fs::read(build_info.out.join(&local)).unwrap(),
            asset.bytes
        );
        assert_eq!(published_media(&build_info.out).get(&local), Some(&hash));
        let markdown =
            std::fs::read_to_string(build_info.out.join("items/blog/2026-09-01-post-0.md"))
                .unwrap();
        assert!(markdown.contains("*[Open PDF](<http://publisher.invalid/paper.pdf#page=2>)*"));
        config.site.build_max_bytes =
            budget::output_bytes(&build_info.out).unwrap() - asset.bytes.len() as u64 / 2;
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        assert!(!build_info.out.join(&local).exists());
        let page = std::fs::read_to_string(build_info.out.join(page_path)).unwrap();
        assert!(page.contains("data=\"http://publisher.invalid/paper.pdf#page=2\""));
        assert!(published_media(&build_info.out).is_empty());
        assert_eq!(
            store
                .read_document(&store.items().unwrap()[0])
                .unwrap()
                .unwrap(),
            asset
        );
        let mut changed = store.items().unwrap().remove(0);
        changed.front.link = "http://publisher.invalid/other.pdf?version=2".into();
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &changed.front,
                body: &changed.body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        config.site.build_max_bytes = 1_000_000_000;
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        assert!(
            !build_info.out.join(&local).exists(),
            "old PDF never replaces a changed document URL"
        );
        let page = std::fs::read_to_string(build_info.out.join(page_path)).unwrap();
        assert!(page.contains("data=\"http://publisher.invalid/other.pdf?version=2\""));
    }

    #[test]
    fn subscription_wall_build_keeps_an_honest_archive_lookup_without_sales_offers() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        item.front.link = "https://www.ft.com/content/123".into();
        item.front.content = crate::model::ContentKind::Extracted;
        item.front.summary = Some("Article URL: https://www.ft.com/content/123 Comments URL: https://news.ycombinator.com/item?id=123 Points: 42 # Comments: 2".into());
        item.front
            .extra
            .insert("archive_lookup_url".into(), "javascript:alert(1)".into());
        let body = "## Save 50% on Standard Digital\n\nExplore more offers.\n\nPremium Digital\n\nComplete digital access.\n\nExplore our full range of subscriptions.";
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let page =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(page.contains("article-access-notice"), "{page}");
        assert!(page.contains("https://archive.ph/https://www.ft.com/content/123"));
        for forbidden in [
            "Save 50%",
            "Premium Digital",
            "Article URL:",
            "javascript:alert",
            "This source publishes titles only",
        ] {
            assert!(!page.contains(forbidden), "{forbidden}");
        }
        assert_eq!(store.items().unwrap()[0].body, body);
    }

    #[test]
    fn pdf_articles_embed_documents_with_fallback_without_loading_them_in_feeds() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        item.front.link =
            "https://example.com/paper.PDF?token=one&filename=paper.pdf#page=2".into();
        item.front.content = crate::model::ContentKind::Feed;
        let summary = format!(
            "Article URL: {} Comments URL: https://news.ycombinator.com/item?id=49783495 Points: 120 # Comments: 21",
            item.front.link
        );
        item.front.summary = Some(summary);
        let feed_body = format!(
            "Article URL: [{}]({})\n\nComments URL: https://news.ycombinator.com/item?id=49783495\n\nPoints: 120\n\n\\# Comments: 21\n",
            item.front.link, item.front.link
        );
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: &feed_body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let page =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(
            !page.contains("Article URL:"),
            "feed bookkeeping leaked into PDF page"
        );
        assert!(!page.contains("Comments URL:"));
        assert!(!page.contains("Points:"));
        assert!(!page.contains("# Comments:"));
        assert_eq!(
            store.items().unwrap()[0].body,
            feed_body,
            "build does not rewrite archive"
        );
        let document = scraper::Html::parse_document(&page);
        let viewer = document
            .select(&scraper::Selector::parse("object.document-viewer").unwrap())
            .next()
            .expect("native PDF object has an HTML fallback when loading is blocked");
        assert_eq!(viewer.value().attr("data"), Some(item.front.link.as_str()));
        let fallback = viewer
            .select(&scraper::Selector::parse(".document-fallback a").unwrap())
            .next()
            .expect("failed PDF has an actionable inline fallback without JavaScript");
        assert_eq!(
            fallback.value().attr("href"),
            Some(item.front.link.as_str())
        );
        assert_eq!(fallback.value().attr("target"), Some("_blank"));
        assert!(
            viewer
                .text()
                .collect::<String>()
                .contains("PDF preview unavailable")
        );
        assert!(page.contains("type=\"application/pdf\""), "{page}");
        assert!(
            page.contains(
                "data=\"https://example.com/paper.PDF?token=one&amp;filename=paper.pdf#page=2\""
            ),
            "{page}"
        );
        assert!(page.contains("Open PDF"), "{page}");
        assert!(page.contains(">Open PDF ↗</a></figcaption>"), "{page}");
        let caption = format!("*[Open PDF](<{}>)*", item.front.link);
        let markdown =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0.md")).unwrap();
        assert!(
            markdown.contains(&caption),
            "portable Markdown carries the media caption: {markdown}"
        );
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(out.join("items/blog/2026-09-01-post-0.json")).unwrap(),
        )
        .unwrap();
        assert!(
            json["content_markdown"]
                .as_str()
                .unwrap()
                .contains(&caption),
            "JSON Markdown carries the same media caption: {json}"
        );
        assert!(!page.contains("if the viewer is unavailable"), "{page}");
        assert!(
            !page.contains("This source publishes titles only"),
            "{page}"
        );
        let river = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(!river.contains("<embed"), "{river}");
        assert!(!river.contains("<object"), "{river}");
    }
    #[test]
    fn reading_pages_advertise_root_feeds_and_keep_microformats_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 2, "");
        for mut item in store.items().unwrap() {
            item.front.authors = vec!["Ada Lovelace".into(), "Grace Hopper".into()];
            let (directory, stem) = item.path.rsplit_once('/').unwrap();
            store
                .write_item(crate::store::NewItem {
                    dir: directory,
                    stem,
                    front: &item.front,
                    body: &item.body,
                    html: None,
                    preview: None,
                    images: &[],
                })
                .unwrap();
        }
        store
            .write_source_state(
                "blog",
                &crate::store::SourceState {
                    resolved_url: Some("https://blog.example/feed.xml".into()),
                    site_url: Some("https://blog.example/".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let parse = |path: &str| {
            scraper::Html::parse_document(&std::fs::read_to_string(out.join(path)).unwrap())
        };
        let select = |selector: &str| scraper::Selector::parse(selector).unwrap();

        let article = parse("items/blog/2026-09-02-post-1/index.html");
        let atom = article
            .select(&select(
                "link[rel=\"alternate\"][type=\"application/atom+xml\"]",
            ))
            .next()
            .expect("article pages advertise the root Atom feed");
        assert_eq!(atom.value().attr("href"), Some("../../../atom.xml"));
        assert_eq!(atom.value().attr("title"), Some("Demo <site>"));
        assert_eq!(
            article
                .select(&select(
                    "link[rel=\"alternate\"][type=\"application/rss+xml\"]"
                ))
                .next()
                .and_then(|link| link.value().attr("href")),
            Some("../../../rss.xml")
        );
        assert_eq!(
            article
                .select(&select("link[rel=\"alternate\"][type=\"text/x-opml\"]"))
                .next()
                .and_then(|link| link.value().attr("href")),
            Some("../../../sources.opml")
        );
        assert_eq!(
            article
                .select(&select("meta[property=\"og:locale\"]"))
                .next()
                .and_then(|meta| meta.value().attr("content")),
            Some("en")
        );
        for utility in ["404.html", "offline.html"] {
            let document = parse(utility);
            assert!(
                document
                    .select(&select("link[rel=\"alternate\"]"))
                    .next()
                    .is_none(),
                "{utility} advertises no feed"
            );
        }
        let river = parse("index.html");
        let feed_name = river
            .select(&select("main.h-feed > data.p-name"))
            .next()
            .expect("the h-feed names itself");
        assert!(feed_name.value().attr("hidden").is_some());
        assert!(
            !feed_name
                .value()
                .attr("value")
                .unwrap_or_default()
                .is_empty()
        );
        assert_eq!(
            river.select(&select("main.h-feed > data.p-name")).count(),
            1
        );
        assert!(
            parse("items/blog/2026-09-02-post-1/index.html")
                .select(&select("data.p-name"))
                .next()
                .is_none(),
            "article pages are not feeds"
        );

        let entry = article
            .select(&select("article.h-entry"))
            .next()
            .expect("article root");
        let url = entry
            .select(&select(":scope > a.u-uid.u-url"))
            .next()
            .expect("the entry names its own URL");
        assert_eq!(
            url.value().attr("href"),
            Some("../../../items/blog/2026-09-02-post-1/")
        );
        let authors = entry
            .select(&select(":scope > data.p-author.h-card"))
            .collect::<Vec<_>>();
        assert_eq!(
            authors
                .iter()
                .map(|author| author.value().attr("value").unwrap())
                .collect::<Vec<_>>(),
            ["Ada Lovelace", "Grace Hopper"]
        );
        assert!(
            entry
                .select(&select(".article-more-card.h-entry"))
                .next()
                .is_some(),
            "related cards are their own entries"
        );
        let outside_cards = |property: &str| {
            entry.select(&select(property)).count()
                - entry
                    .select(&select(&format!(".article-more-card.h-entry {property}")))
                    .count()
        };
        assert_eq!(outside_cards(".dt-published"), 1);
        assert_eq!(outside_cards(".u-bookmark-of"), 1);
        assert_eq!(outside_cards(".p-name"), 1);
        for added in entry
            .select(&select(":scope > a.u-url, :scope > data.p-author"))
            .chain(river.select(&select("main > data.p-name")))
        {
            assert!(added.value().attr("hidden").is_some(), "{}", added.html());
            assert!(
                added.text().collect::<String>().is_empty(),
                "{}",
                added.html()
            );
        }
        let glyph = select("a.config-link > span[aria-hidden=\"true\"]");
        let config_link = article.select(&select("a.config-link")).next().unwrap();
        assert_eq!(config_link.text().collect::<String>(), "aggr.toml ↗");
        assert_eq!(article.select(&glyph).next().unwrap().inner_html(), "↗");
        assert_eq!(
            river
                .select(&select("label[for=\"q\"]"))
                .next()
                .unwrap()
                .text()
                .collect::<String>(),
            "Search articles"
        );
        assert_eq!(
            parse("browse/index.html")
                .select(&select("ul.browse-entries"))
                .next()
                .and_then(|list| list.value().attr("role")),
            Some("list")
        );

        let opml = std::fs::read_to_string(out.join("sources.opml")).unwrap();
        let imported = Config::parse_source_document(
            opml.as_bytes(),
            &url::Url::parse("https://u.github.io/repo/sources.opml").unwrap(),
        )
        .unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(
            imported[0].url.as_deref(),
            Some("https://blog.example/feed.xml"),
            "the resolved endpoint wins over the configured URL"
        );
        assert!(opml.contains("htmlUrl=\"https://blog.example/\""), "{opml}");
        let descriptor: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out.join("aggr.json")).unwrap()).unwrap();
        assert_eq!(
            descriptor["discovery"]["opml"],
            "https://u.github.io/repo/sources.opml"
        );
        assert!(
            std::fs::read_to_string(out.join("llms.txt"))
                .unwrap()
                .contains("https://u.github.io/repo/sources.opml")
        );
        let stub_source = std::fs::read_to_string(out.join("sw.js")).unwrap();
        assert!(
            !stub_source.contains("sources.opml"),
            "only content-addressed URLs are cached first; the list needs no special case"
        );
    }
}
