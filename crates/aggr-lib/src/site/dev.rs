//! Request-driven development rendering. The archive's display metadata is shared, while pages,
//! media and the search index are materialized only when a browser asks for them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock, Weak};

use anyhow::Result;

use super::page::{Collection, FIXED_PAGES};
use super::*;

#[derive(Default)]
pub(crate) struct PreparationCache {
    normalized: Mutex<BTreeMap<String, (String, Item)>>,
    markdown: Mutex<BTreeMap<String, Arc<content::PreparedMarkdown>>>,
    prepared: Mutex<Option<(String, PreparedSite)>>,
    cold_work: Arc<ColdWork>,
}

struct ColdWork {
    capacity: usize,
    active: Mutex<usize>,
    available: Condvar,
}

impl Default for ColdWork {
    fn default() -> Self {
        // Background sync reserves the other half. Generations share this same budget so an
        // old snapshot finishing a request cannot double the new snapshot's CPU allowance.
        Self::new((parallel::workers() / 2).max(1))
    }
}

impl ColdWork {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            active: Mutex::new(0),
            available: Condvar::new(),
        }
    }

    fn acquire(&self) -> ColdPermit<'_> {
        let mut active = lock(&self.active);
        while *active >= self.capacity {
            active = self
                .available
                .wait(active)
                .unwrap_or_else(PoisonError::into_inner);
        }
        *active += 1;
        ColdPermit(self)
    }
}

struct ColdPermit<'a>(&'a ColdWork);

impl Drop for ColdPermit<'_> {
    fn drop(&mut self) {
        *lock(&self.0.active) -= 1;
        self.0.available.notify_one();
    }
}

impl PreparationCache {
    pub(super) fn prepared(&self, key: &str) -> Option<PreparedSite> {
        lock(&self.prepared)
            .as_ref()
            .filter(|(stored, _)| stored == key)
            .map(|(_, prepared)| prepared.clone())
    }

    pub(super) fn remember(&self, key: String, prepared: &PreparedSite) {
        *lock(&self.prepared) = Some((key, prepared.clone()));
    }
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(super) fn normalize_item(item: &Item, cache: Option<&PreparationCache>) -> Item {
    let key = cache.map(|_| crate::model::sha1_hex(format!("{:?}", item).as_bytes()));
    if let Some(value) = cache.and_then(|cache| {
        lock(&cache.normalized)
            .get(&item.path)
            .filter(|(fingerprint, _)| Some(fingerprint) == key.as_ref())
            .map(|(_, item)| item.clone())
    }) {
        return value;
    }
    let mut normalized = item.clone();
    let front = &mut normalized.front;
    let body = content::normalize_subscription_metadata(&item.body, front);
    let body = content::normalize_aggregator_metadata(&body, front);
    let (body, labels) =
        content::normalize_article_body(&body, &front.title, front.published, &front.source);
    front.labels = crate::model::normalize_labels(front.labels.iter().chain(&labels));
    normalized.body = crate::threads::clean_archived_thread(&body, &front.link);
    if let (Some(cache), Some(key)) = (cache, key) {
        lock(&cache.normalized).insert(item.path.clone(), (key, normalized.clone()));
    }
    normalized
}

pub(super) fn prepare_markdown(
    body: &str,
    cache: Option<&PreparationCache>,
    persistent: Option<&Path>,
) -> Arc<content::PreparedMarkdown> {
    let key = cache.map(|_| crate::model::sha1_hex(body.as_bytes()));
    if let Some(value) = cache.and_then(|cache| {
        key.as_ref()
            .and_then(|key| lock(&cache.markdown).get(key).cloned())
    }) {
        return value;
    }
    let prepared = Arc::new(match persistent {
        Some(root) => content::PreparedMarkdown::cached(body, root),
        None => content::PreparedMarkdown::new(body),
    });
    if let (Some(cache), Some(key)) = (cache, key) {
        lock(&cache.markdown).insert(key, prepared.clone());
    }
    prepared
}

pub(crate) struct Site {
    prepared: Arc<PreparedSite>,
    ui: Ui,
    store: Store,
    media_policy: assets::MediaPolicy,
    full_quality_days: u32,
    sources: Vec<Source>,
    info: BuildInfo,
    articles: BTreeMap<String, usize>,
    shared_previews: std::collections::BTreeSet<usize>,
    shared_images: BTreeMap<usize, std::collections::BTreeSet<String>>,
    collections: Vec<Collection>,
    lists: BTreeMap<String, (usize, Pager)>,
    media_memo: assets::MediaMemo,
    shared: SharedCtx,
    search: Mutex<bool>,
    search_source: Option<Arc<Site>>,
    catalogue: pagefind::SearchCatalogue,
    offline: Mutex<OfflineState>,
    // Media preparation is isolated per article. Only final deduplicated publication shares a
    // short lock; ready responses never wait for preparation or rendering.
    materialized: Vec<Mutex<Materialized>>,
    published: Mutex<assets::Published>,
    routes: Mutex<BTreeMap<String, Weak<Mutex<()>>>>,
    ready: RwLock<BTreeSet<String>>,
    cold_work: Arc<ColdWork>,
    directory: Option<tempfile::TempDir>,
    archive_directory: Option<Arc<tempfile::TempDir>>,
}

#[derive(Clone)]
struct Ui {
    renderer: Renderer,
    build_ctx: BuildCtx,
}

#[derive(Default)]
enum OfflineState {
    #[default]
    Idle,
    Building,
    Ready,
    Failed(String),
}

#[derive(Default)]
struct Materialized {
    preview: Option<Option<context::PreviewCtx>>,
    media: Option<(ItemCtx, Vec<content::LocalImage>)>,
}

/// Stored companion names already carry immutable content hashes. Counting these requires no
/// image reads and gives lazy pages the same repeated-publisher-picture suppression as a build.
fn shared_media(
    prepared: &PreparedSite,
) -> (
    std::collections::BTreeSet<usize>,
    BTreeMap<usize, std::collections::BTreeSet<String>>,
) {
    let mut previews = BTreeMap::<(&str, &str), usize>::new();
    let mut pictures = BTreeMap::<(&str, &str), usize>::new();
    for (item, body) in prepared.all_items.iter().zip(&prepared.prepared_bodies) {
        if let Some(preview) = &item.front.preview
            && let Some((_, digest)) = preview.file.rsplit_once(".preview-")
        {
            *previews.entry((&item.front.source, digest)).or_default() += 1;
        }
        let hashes: std::collections::BTreeSet<_> = item
            .front
            .images
            .iter()
            .filter(|image| !body.portable_html().contains(image.source.as_str()))
            .filter_map(|image| {
                image
                    .original
                    .file
                    .rsplit_once(".image-")
                    .map(|(_, digest)| digest)
            })
            .collect();
        for digest in hashes {
            *pictures.entry((&item.front.source, digest)).or_default() += 1;
        }
    }
    let mut hidden_previews = std::collections::BTreeSet::new();
    let mut hidden_images = BTreeMap::new();
    for (index, item) in prepared.all_items.iter().enumerate() {
        if item
            .front
            .preview
            .as_ref()
            .and_then(|preview| preview.file.rsplit_once(".preview-"))
            .and_then(|(_, digest)| previews.get(&(item.front.source.as_str(), digest)))
            .is_some_and(|&count| count >= SHARED_PICTURE_ARTICLES)
        {
            hidden_previews.insert(index);
        }
        let sources: std::collections::BTreeSet<_> = item
            .front
            .images
            .iter()
            .filter(|image| {
                !prepared.prepared_bodies[index]
                    .portable_html()
                    .contains(image.source.as_str())
                    && image
                        .original
                        .file
                        .rsplit_once(".image-")
                        .and_then(|(_, digest)| pictures.get(&(item.front.source.as_str(), digest)))
                        .is_some_and(|&count| count >= SHARED_PICTURE_ARTICLES)
            })
            .map(|image| image.source.clone())
            .collect();
        if !sources.is_empty() {
            hidden_images.insert(index, sources);
        }
    }
    (hidden_previews, hidden_images)
}

impl Site {
    pub(crate) fn prepare(
        config: &Config,
        sources: &[Source],
        store: Store,
        project_root: &Path,
        info: BuildInfo,
        cache: &PreparationCache,
    ) -> Result<Self> {
        let started = std::time::Instant::now();
        let prepared = prepare_site(
            config,
            sources,
            &store,
            project_root,
            &info,
            &mut budget::MediaBudget::new(u64::MAX),
            Some(cache),
        )?;
        if let Some(root) = &info.pagefind_cache
            && root.exists()
            && let Err(error) = crate::cache::enforce_limits(root, &config.cache)
        {
            log::warn!("dev cache limit cleanup skipped: {error:#}");
        }
        // Keep only the current archive in the hot-reload cache. Deleted or replaced content must
        // not accumulate for the lifetime of a long-running development process.
        let bodies: std::collections::BTreeSet<_> = prepared
            .all_items
            .iter()
            .map(|item| crate::model::sha1_hex(item.body.as_bytes()))
            .collect();
        lock(&cache.markdown).retain(|key, _| bodies.contains(key));
        let paths: std::collections::BTreeSet<_> = prepared
            .all_items
            .iter()
            .map(|item| item.path.as_str())
            .collect();
        lock(&cache.normalized).retain(|path, _| paths.contains(path.as_str()));
        let ui = Ui {
            renderer: prepared.renderer.clone(),
            build_ctx: prepared.build_ctx.clone(),
        };
        Self::from_prepared(
            config,
            sources,
            store,
            info,
            cache,
            (Arc::new(prepared), ui),
            started,
        )
    }

    fn from_prepared(
        config: &Config,
        sources: &[Source],
        store: Store,
        info: BuildInfo,
        cache: &PreparationCache,
        prepared: (Arc<PreparedSite>, Ui),
        started: std::time::Instant,
    ) -> Result<Self> {
        let (prepared, ui) = prepared;
        ui.renderer.write_static(&info.out)?;
        write(&info.out.join(MARKER), env!("CARGO_PKG_VERSION").as_bytes())?;
        write(
            &info.out.join("updates.json"),
            outputs::updates(&prepared.site, &ui.build_ctx)?.as_bytes(),
        )?;
        let articles: BTreeMap<_, _> = prepared
            .archive_items
            .iter()
            .enumerate()
            .map(|(i, item)| (item.url.clone(), i))
            .collect();
        let collections = prepared.collections();
        let lists = collections
            .iter()
            .enumerate()
            .flat_map(|(i, collection)| {
                paginate(
                    &collection.prefix,
                    collection.indices.len(),
                    prepared.per_page,
                )
                .into_iter()
                .map(move |pager| (format!("{}index.html", pager.path), (i, pager)))
            })
            .collect();
        log::info!(
            "dev: prepared {} article routes in {:.3}s; pages, media and search render on demand",
            articles.len(),
            started.elapsed().as_secs_f64()
        );
        let media_memo = assets::MediaMemo::new(store.image_cache().cloned());
        // Dev generations have private immutable index URLs. A catalogue can therefore name a
        // deferred index before Pagefind runs, without exposing mixed chunks during a reload.
        use sha2::{Digest as _, Sha256};
        let index_version = hex::encode(Sha256::digest(ui.build_ctx.content_version.as_bytes()));
        let vocabulary: Vec<_> = prepared
            .archive_items
            .iter()
            .map(|ctx| pagefind::SearchDocument::new(ctx, ""))
            .collect();
        let catalogue = pagefind::catalogue(index_version, &vocabulary);
        write(
            &info.out.join("search-catalog.json"),
            &serde_json::to_vec(&catalogue)?,
        )?;
        let (shared_previews, shared_images) = shared_media(&prepared);
        let shared = SharedCtx::new(
            &prepared.site,
            &ui.build_ctx,
            &prepared.source_ctxs,
            &prepared.categories,
            &prepared.tags,
        );
        let materialized = (0..prepared.all_items.len())
            .map(|_| Mutex::default())
            .collect();
        let ready = walkdir::WalkDir::new(&info.out)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .filter_map(|entry| {
                entry
                    .path()
                    .strip_prefix(&info.out)
                    .ok()
                    .map(|path| path.to_string_lossy().into_owned())
            })
            .collect();
        let site = Self {
            prepared,
            ui,
            store,
            media_policy: assets::MediaPolicy::defaults(&config.defaults)
                .with_image_format(config.site.media_image_format),
            full_quality_days: config.site.media_full_quality_days,
            sources: sources.to_vec(),
            info,
            articles,
            shared_previews,
            shared_images,
            collections,
            lists,
            media_memo,
            shared,
            search: Mutex::new(false),
            search_source: None,
            catalogue,
            offline: Mutex::default(),
            materialized,
            published: Mutex::default(),
            routes: Mutex::default(),
            ready: RwLock::new(ready),
            cold_work: Arc::clone(&cache.cold_work),
            directory: None,
            archive_directory: None,
        };
        if config.site.pwa {
            site.prepare_shell(config.preferences.offline_items)?;
        }
        Ok(site)
    }

    /// A template edit replaces only page/asset outputs, retaining immutable data and search.
    pub(crate) fn reload_ui(
        self: &Arc<Self>,
        config: &Config,
        cache: &PreparationCache,
    ) -> Result<Option<Self>> {
        self.reload_ui_with_renderer(config, cache, Renderer::new()?)
    }

    fn reload_ui_with_renderer(
        self: &Arc<Self>,
        config: &Config,
        cache: &PreparationCache,
        renderer: Renderer,
    ) -> Result<Option<Self>> {
        if renderer.hash() == self.ui.renderer.hash() {
            return Ok(None);
        }
        let parent = self.root().parent().context("preview generation parent")?;
        let directory = tempfile::Builder::new()
            .prefix("ui-snapshot-")
            .tempdir_in(parent)?;
        let mut info = self.info.clone();
        info.out = directory.path().join("site");
        prepare_out_dir(&info.out)?;
        if self.root().join("aggr.toml").is_file() {
            std::fs::copy(self.root().join("aggr.toml"), info.out.join("aggr.toml"))?;
        }
        let mut ui = self.ui.clone();
        ui.build_ctx.app_version = app_version_for_ui(env!("CARGO_PKG_VERSION"), renderer.hash());
        ui.renderer = renderer;
        let mut site = Self::from_prepared(
            config,
            &self.sources,
            self.store.clone(),
            info,
            cache,
            (Arc::clone(&self.prepared), ui),
            std::time::Instant::now(),
        )?
        .own_directory(directory);
        site.archive_directory = self.archive_directory.clone();
        site.search_source = Some(self.search_source.as_ref().unwrap_or(self).clone());
        Ok(Some(site))
    }

    fn prepare_shell(&self, offline_count: usize) -> Result<()> {
        let pages = self.pages();
        write(
            &self.root().join("offline.html"),
            pages
                .simple(SimplePage {
                    page_items: Some(
                        &self.prepared.archive_items
                            [..self.prepared.archive_items.len().min(offline_count)],
                    ),
                    ..SimplePage::new("offline", "Offline", "offline.html")
                })?
                .as_bytes(),
        )?;
        write(
            &self.root().join("manifest.webmanifest"),
            pages.manifest()?.as_bytes(),
        )?;
        self.mark_ready(["offline.html".to_owned(), "manifest.webmanifest".to_owned()].into_iter());
        for key in ["browse/index.html", "preferences/index.html", "404.html"] {
            self.response(key)?.context("preparing offline shell")?;
        }
        let renderer = &self.ui.renderer;
        let assets = renderer.write_static(self.root())?;
        let paths = precache::paths(renderer, &assets);
        let mut config = SwConfig {
            version: cache_version(&self.ui.build_ctx),
            app_version: self.ui.build_ctx.app_version.clone(),
            content_version: self.ui.build_ctx.content_version.clone(),
            precache: assets::precache_entries(self.root(), paths, &lock(&self.published))?,
            offline_catalog_url: "offline-catalog.json".into(),
            offline_count,
            search_manifest: serde_json::json!({"version":self.catalogue.version,"base":self.catalogue.base}),
        };
        let worker = renderer.asset_site_path("assets/app/worker.js");
        config.version = crate::model::sha1_hex(worker_source(&config, &worker)?.as_bytes());
        write(
            &self.root().join("sw.js"),
            worker_source(&config, &worker)?.as_bytes(),
        )?;
        self.mark_ready(std::iter::once("sw.js".to_owned()));
        Ok(())
    }

    /// Expensive offline families are single-flight and survive a disconnected catalogue request.
    pub(crate) fn start_offline_catalogue(self: &Arc<Self>) -> Result<bool> {
        if !self.prepared.site.pwa {
            return Ok(true);
        }
        let mut state = lock(&self.offline);
        match &*state {
            OfflineState::Ready => return Ok(true),
            OfflineState::Building => return Ok(false),
            OfflineState::Failed(error) => {
                let error = anyhow::anyhow!("offline preparation failed: {error}");
                *state = OfflineState::Idle;
                return Err(error);
            }
            OfflineState::Idle => {}
        }
        *state = OfflineState::Building;
        let site = Arc::clone(self);
        if let Err(error) = std::thread::Builder::new()
            .name("aggr-dev-offline".into())
            .spawn(move || {
                let result = site.prepare_offline_catalogue();
                *lock(&site.offline) = match result {
                    Ok(()) => OfflineState::Ready,
                    Err(error) => OfflineState::Failed(format!("{error:#}")),
                };
            })
        {
            *state = OfflineState::Idle;
            return Err(error.into());
        }
        Ok(false)
    }

    fn prepare_offline_catalogue(&self) -> Result<()> {
        let mut items = Vec::new();
        let mut images = BTreeMap::new();
        for index in 0..self.prepared.archive_items.len().min(1000) {
            let key = format!("{}index.html", self.prepared.archive_items[index].url);
            self.response(&key)?.context("rendering offline article")?;
            let (item, local) = self.with_media(index)?;
            images.insert(item.path.clone(), local);
            items.push(item);
        }
        let catalogue = offline::catalogue(self.root(), &items, &images, &lock(&self.published))?;
        write(
            &self.root().join("offline-catalog.json"),
            &serde_json::to_vec(&catalogue)?,
        )?;
        self.mark_ready(std::iter::once("offline-catalog.json".to_owned()));
        Ok(())
    }

    pub(crate) fn root(&self) -> &Path {
        &self.info.out
    }

    pub(crate) fn own_directory(mut self, directory: tempfile::TempDir) -> Self {
        self.directory = Some(directory);
        self
    }

    pub(crate) fn own_archive(mut self, directory: Arc<tempfile::TempDir>) -> Self {
        self.archive_directory = Some(directory);
        self
    }

    fn pages(&self) -> Pages<'_> {
        let prepared = &self.prepared;
        Pages::new(
            &prepared.site,
            &self.ui.renderer,
            self.shared.clone(),
            &prepared.archive_items,
            prepared.per_page,
        )
    }

    fn with_preview(&self, index: usize) -> Result<ItemCtx> {
        self.with_preview_locked(index, &mut lock(&self.materialized[index]))
    }

    fn with_preview_locked(&self, index: usize, state: &mut Materialized) -> Result<ItemCtx> {
        let mut ctx = self.prepared.archive_items[index].clone();
        if self.shared_previews.contains(&index) {
            return Ok(ctx);
        }
        let preview = match &state.preview {
            Some(preview) => preview.clone(),
            None => {
                let item = &self.prepared.all_items[index];
                let policy = self.media_policy.for_item(&self.sources, item);
                let compact_cache = self
                    .info
                    .pagefind_cache
                    .as_deref()
                    .map(|root| crate::cache::Namespace::DeploymentMedia.dir(root));
                let media = assets::ItemMedia::gather_preview(
                    &self.store,
                    item,
                    policy,
                    &self.media_memo,
                    compact_cache.as_deref(),
                )?;
                let (preview, _, _) = self.publish_media(media)?;
                state.preview = Some(preview.clone());
                preview
            }
        };
        ctx.preview = preview;
        Ok(ctx)
    }

    fn with_media(&self, index: usize) -> Result<(ItemCtx, Vec<content::LocalImage>)> {
        let mut state = lock(&self.materialized[index]);
        if let Some(media) = &state.media {
            return Ok(media.clone());
        }
        let mut ctx = self.with_preview_locked(index, &mut state)?;
        let item = &self.prepared.all_items[index];
        let compact_cache = self
            .info
            .pagefind_cache
            .as_deref()
            .map(|root| crate::cache::Namespace::DeploymentMedia.dir(root));
        let media = assets::ItemMedia::gather(
            &self.store,
            item,
            self.media_policy.for_item(&self.sources, item),
            &self.media_memo,
            !budget::full_quality(item.created_at(), self.info.now, self.full_quality_days),
            compact_cache.as_deref(),
        )?;
        let (_, images, document) = self.publish_media(media)?;
        if let Some(ctx) = &mut ctx.document {
            ctx.local_url = document.map(|mut local| {
                if let Ok(url) = url::Url::parse(&ctx.url)
                    && let Some(fragment) = url.fragment()
                {
                    local.push('#');
                    local.push_str(fragment);
                }
                local
            });
        }
        state.media = Some((ctx.clone(), images.clone()));
        Ok((ctx, images))
    }

    fn is_ready(&self, key: &str) -> bool {
        self.ready
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(key)
    }

    fn mark_ready(&self, keys: impl Iterator<Item = String>) {
        self.ready
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(keys);
    }

    fn family(&self, key: &str) -> String {
        if let Some(path) = key.strip_suffix("index.html")
            && self.articles.contains_key(path)
        {
            return path.to_owned();
        }
        if let Some((stem, extension)) = key.rsplit_once('.')
            && matches!(extension, "md" | "txt" | "rst" | "json")
            && self.articles.contains_key(&format!("{stem}/"))
        {
            return format!("{stem}/");
        }
        if let Some((prefix, name)) = key.rsplit_once('/')
            && matches!(name, "feed.xml" | "atom.xml" | "rss.xml" | "feed.json")
        {
            return format!("{prefix}/feeds");
        }
        if matches!(key, "feed.xml" | "atom.xml" | "rss.xml" | "feed.json") {
            return "feeds".to_owned();
        }
        key.to_owned()
    }

    fn publish_media(
        &self,
        media: assets::ItemMedia,
    ) -> Result<(
        Option<context::PreviewCtx>,
        Vec<content::LocalImage>,
        Option<String>,
    )> {
        let mut published = lock(&self.published);
        let result = media.publish(
            self.root(),
            &mut published,
            &mut budget::MediaBudget::new(u64::MAX),
        )?;
        let paths = result
            .0
            .iter()
            .map(|preview| preview.url.clone())
            .chain(result.1.iter().flat_map(|image| {
                std::iter::once(image.original.clone())
                    .chain(image.variants.iter().map(|variant| variant.url.clone()))
            }))
            .chain(result.2.iter().cloned())
            .filter(|path| published.contains_key(path));
        self.mark_ready(paths);
        Ok(result)
    }

    pub(crate) fn response(&self, key: &str) -> Result<Option<Vec<u8>>> {
        if key.is_empty()
            || key.contains('\\')
            || Path::new(key)
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Ok(None);
        }
        if key.starts_with(&self.catalogue.base) || key == "search-manifest.json" {
            if let Some(source) = &self.search_source {
                return source.response(key);
            }
            self.prepare_search()?;
            return match std::fs::read(self.root().join(key)) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.into()),
            };
        }
        // Only completed generated families and embedded static assets are visible. Presence on
        // disk alone is insufficient while a different request is still publishing companions.
        if key.starts_with('.') {
            return Ok(None);
        }
        if !self.is_ready(key) {
            let family = self.family(key);
            let route = {
                let mut routes = lock(&self.routes);
                routes.retain(|_, route| route.strong_count() > 0);
                match routes.get(&family).and_then(Weak::upgrade) {
                    Some(route) => route,
                    None => {
                        let route = Arc::new(Mutex::new(()));
                        routes.insert(family, Arc::downgrade(&route));
                        route
                    }
                }
            };
            let _render = lock(&route);
            if !self.is_ready(key) {
                let _permit = self.cold_work.acquire();
                if !self.is_ready(key) && !self.materialize(key)? {
                    return Ok(None);
                }
                self.mark_ready(std::iter::once(key.to_owned()));
            }
        }
        let path = self.root().join(key);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn prepare_search(&self) -> Result<()> {
        let mut built = lock(&self.search);
        if *built {
            return Ok(());
        }
        let _permit = self.cold_work.acquire();
        let documents = (0..self.prepared.archive_items.len())
            .map(|index| {
                let ctx = self.with_preview(index)?;
                Ok(pagefind::SearchDocument::new(
                    &ctx,
                    self.prepared.prepared_bodies[index].plain_text(),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let staging = tempfile::Builder::new()
            .prefix(".search-")
            .tempdir_in(self.root())?;
        let published = pagefind::build_cached(
            staging.path(),
            &documents,
            &self.prepared.site.language,
            self.info.pagefind_cache.as_deref(),
        )?;
        std::fs::create_dir_all(self.root().join("pagefind"))?;
        std::fs::rename(
            staging.path().join(published.base),
            self.root().join(&self.catalogue.base),
        )?;
        offline::search_manifest(self.root(), &self.catalogue)?;
        self.mark_ready(std::iter::once("search-manifest.json".to_owned()));
        *built = true;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn page_model(&self, key: &str) -> Result<Option<aggr_ui_model::RenderRequest>> {
        self.page_request(key)
    }

    fn page_request(&self, key: &str) -> Result<Option<aggr_ui_model::RenderRequest>> {
        let prepared = &self.prepared;
        if let Some((collection, pager)) = self.lists.get(key) {
            let collection = &self.collections[*collection];
            let mut items: Vec<_> = collection
                .indices
                .iter()
                .map(|&i| prepared.archive_items[i].clone())
                .collect();
            for index in pager.range.clone() {
                items[index] = self.with_preview(collection.indices[index])?;
            }
            let page = collection.page(
                &items,
                &prepared.source_ctxs,
                &prepared.categories,
                &prepared.tags,
            );
            return Ok(Some(self.pages().list_request(&page, pager)));
        }
        if let Some(index) = key
            .strip_suffix("index.html")
            .and_then(|path| self.articles.get(path))
            .copied()
        {
            let (ctx, images) = self.with_media(index)?;
            let item = &prepared.all_items[index];
            let hidden = images
                .iter()
                .filter(|image| {
                    self.shared_images
                        .get(&index)
                        .is_some_and(|sources| sources.contains(&image.source))
                })
                .flat_map(|image| {
                    std::iter::once(image.original.clone())
                        .chain(image.variants.iter().map(|variant| variant.url.clone()))
                })
                .collect();
            let shared = BTreeMap::from([(ctx.source.clone(), hidden)]);
            let ctx = prepare_article(
                &prepared.site,
                &self.store,
                &ctx,
                item,
                &prepared.prepared_bodies[index],
                &BTreeMap::from([(item.path.clone(), images)]),
                &shared,
            )?;
            return Ok(Some(self.pages().simple_request(SimplePage {
                item: Some(&ctx),
                ..SimplePage::new("item", &ctx.title, &ctx.url)
            })));
        }
        if let Some((page, _)) = FIXED_PAGES
            .iter()
            .find(|(page, _)| page.output_path() == Path::new(key))
        {
            return Ok(Some(self.pages().simple_request(*page)));
        }
        Ok(None)
    }

    fn materialize(&self, key: &str) -> Result<bool> {
        let prepared = &self.prepared;
        let article_path = key
            .strip_suffix("index.html")
            .map(str::to_owned)
            .or_else(|| {
                key.rsplit_once('.')
                    .filter(|(_, ext)| matches!(*ext, "md" | "txt" | "rst" | "json"))
                    .map(|(stem, _)| format!("{stem}/"))
            });
        if let Some(index) = article_path
            .as_ref()
            .and_then(|path| self.articles.get(path))
            .copied()
        {
            let (ctx, images) = self.with_media(index)?;
            let item = &prepared.all_items[index];
            let hidden: std::collections::BTreeSet<_> = images
                .iter()
                .filter(|image| {
                    self.shared_images
                        .get(&index)
                        .is_some_and(|sources| sources.contains(&image.source))
                })
                .flat_map(|image| {
                    std::iter::once(image.original.clone())
                        .chain(image.variants.iter().map(|variant| variant.url.clone()))
                })
                .collect();
            let shared = BTreeMap::from([(ctx.source.clone(), hidden)]);
            write_article(
                self.root(),
                &self.pages(),
                &prepared.site,
                &self.ui.build_ctx,
                &self.store,
                &ctx,
                item,
                &prepared.prepared_bodies[index],
                &BTreeMap::from([(item.path.clone(), images)]),
                &shared,
            )?;
            let path = self.prepared.archive_items[index].url.trim_end_matches('/');
            let mut family = vec![format!("{path}/index.html")];
            family.extend(
                ["md", "txt", "rst", "json"].map(|extension| format!("{path}.{extension}")),
            );
            self.mark_ready(family.into_iter());
            return Ok(true);
        }
        if let Some(request) = self.page_request(key)? {
            crate::cache::write(
                &self.root().join(key),
                self.ui.renderer.render_page(&request)?.as_bytes(),
            )?;
            return Ok(true);
        }
        if let Some((_, target)) = prepared
            .duplicate_redirects
            .iter()
            .find(|(old, _)| format!("{old}index.html") == key)
        {
            write(
                &self.root().join(key),
                outputs::redirect_stub(&prepared.site, &prepared.site.url(target)).as_bytes(),
            )?;
            return Ok(true);
        }
        for collection in &self.collections {
            if let Some(name) = key.strip_prefix(&collection.prefix)
                && matches!(name, "feed.xml" | "atom.xml" | "rss.xml" | "feed.json")
            {
                let list: Vec<_> = collection
                    .indices
                    .iter()
                    .take(prepared.per_page)
                    .map(|&i| self.with_preview(i))
                    .collect::<Result<_>>()?;
                let bodies = prepared
                    .all_items
                    .iter()
                    .zip(&prepared.prepared_bodies)
                    .map(|(item, body)| (item.path.as_str(), body.as_ref()))
                    .collect();
                let items = feed_items(&list, &bodies, prepared.per_page);
                write_collection_feeds(
                    self.root(),
                    &prepared.site,
                    &self.ui.build_ctx,
                    &collection.title,
                    &collection.prefix,
                    &items,
                )?;
                if collection.prefix.is_empty() {
                    write(
                        &self.root().join("feed.xml"),
                        outputs::atom_feed(&prepared.site, &self.ui.build_ctx, &items).as_bytes(),
                    )?;
                }
                self.mark_ready(
                    ["feed.xml", "atom.xml", "rss.xml", "feed.json"]
                        .into_iter()
                        .map(|name| format!("{}{name}", collection.prefix)),
                );
                return Ok(true);
            }
        }
        let updated = prepared
            .archive_items
            .iter()
            .map(archive_modified_at)
            .max()
            .unwrap_or(self.ui.build_ctx.time);
        let body = match key {
            "aggr.json" => {
                outputs::instance_descriptor(&prepared.site, &self.ui.build_ctx, updated)?
            }
            "llms.txt" => outputs::llms_txt(&prepared.site),
            "sources.opml" => {
                outputs::sources_opml(&prepared.site, updated, &prepared.subscriptions)
            }
            "linkset.json" if prepared.site.base_url.is_some() => {
                outputs::linkset_json(&prepared.site, &prepared.archive_items)?
            }
            "opensearch.xml" if prepared.site.base_url.is_some() => {
                let root = prepared.site.base_url.as_deref().unwrap_or_default();
                outputs::opensearch_description(&outputs::OpenSearchDescription {
                    short_name: &prepared.site.title,
                    description: &prepared.site.description,
                    search_url: &format!("{root}?q={{searchTerms}}"),
                    self_url: Some(&format!("{root}opensearch.xml")),
                })
            }
            "robots.txt" if prepared.site.base_path == "/" => {
                "User-agent: *\nAllow: /\n".to_string()
            }
            ".nojekyll" => String::new(),
            _ => return Ok(false),
        };
        write(&self.root().join(key), body.as_bytes())?;
        Ok(true)
    }
}

#[cfg(test)]
mod benchmarks {
    use super::*;

    /// Reads an existing isolated archive; all generated files and derived-image caches stay in
    /// a temporary directory. Run explicitly with AGGR_BENCH_CONFIG and AGGR_BENCH_DATA set.
    #[tokio::test]
    #[ignore = "requires an existing archive and configuration"]
    async fn cached_archive_prepare_and_first_page() {
        use std::time::Instant;
        let _ = env_logger::builder()
            .filter_module("aggr::site", log::LevelFilter::Debug)
            .is_test(true)
            .try_init();

        let config_path = std::path::PathBuf::from(
            std::env::var_os("AGGR_BENCH_CONFIG").expect("set AGGR_BENCH_CONFIG"),
        )
        .canonicalize()
        .unwrap();
        let data = std::env::var_os("AGGR_BENCH_DATA").expect("set AGGR_BENCH_DATA");
        let config = Config::load_offline(&config_path).await.unwrap();
        let sources = config.sources().unwrap();
        let root = config_path.parent().unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let image_cache = temporary.path().join("cache");
        let store = Store::open(Path::new(&data)).with_image_cache(&image_cache);
        let now = chrono::Utc::now();
        let scan = Instant::now();
        let items = store.items().unwrap();
        let generation = render_generation(&items, &config.site, now);
        eprintln!(
            "archive: {} items; scan {:.3}s",
            items.len(),
            scan.elapsed().as_secs_f64()
        );
        drop(items);
        let uncached_cache = PreparationCache::default();
        let cold_cache = PreparationCache::default();
        let cache = PreparationCache::default();
        for label in ["uncached", "cold", "persistent", "hot"] {
            let info = BuildInfo {
                hermetic: false,
                metrics: Default::default(),
                out: temporary.path().join(label),
                base_url: None,
                config_sha: None,
                config_path: Some("aggr.toml".into()),
                data_sha: None,
                generation: generation.clone(),
                now,

                discussions: crate::discussions::ResolutionSet::default(),
                development: true,
                render_cache_key: None,
                pagefind_cache: (label != "uncached").then(|| image_cache.clone()),
            };
            let started = Instant::now();
            let site = Site::prepare(
                &config,
                &sources,
                Store::open(Path::new(&data)).with_image_cache(&image_cache),
                root,
                info,
                match label {
                    "uncached" => &uncached_cache,
                    "cold" => &cold_cache,
                    _ => &cache,
                },
            )
            .unwrap();
            let preparation = started.elapsed();
            assert!(!site.root().join("index.html").exists());
            assert!(!site.root().join("items").exists());
            assert!(!site.root().join("pagefind").exists());
            let requested = Instant::now();
            let first = site.response("index.html").unwrap().unwrap();
            let first_page = requested.elapsed();
            let repeated = Instant::now();
            let second = site.response("index.html").unwrap().unwrap();
            let cached_page = repeated.elapsed();
            assert_eq!(first, second);
            eprintln!(
                "{label}: prepare {:.3}s; first index {:.3}s; cached index {:.6}s; {} bytes",
                preparation.as_secs_f64(),
                first_page.as_secs_f64(),
                cached_page.as_secs_f64(),
                first.len()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn site(root: &Path) -> Site {
        site_with_cache(
            root,
            &PreparationCache {
                cold_work: Arc::new(ColdWork::new(2)),
                ..Default::default()
            },
        )
    }

    fn site_with_cache(root: &Path, cache: &PreparationCache) -> Site {
        let (config, sources, store) =
            crate::site::tests::fixture(root, 3, "max_age_days = 30\npwa = false\n");
        let mut info = crate::site::tests::info(root.join("lazy"));
        info.development = true;
        Site::prepare(&config, &sources, store, root, info, cache).unwrap()
    }

    #[test]
    fn ui_reload_reuses_archive_preparation_and_search_without_rereading_it() {
        let directory = tempfile::tempdir().unwrap();
        let (config, sources, store) =
            crate::site::tests::fixture(directory.path(), 3, "max_age_days = 30\npwa = false\n");
        let mut info = crate::site::tests::info(directory.path().join("lazy"));
        info.development = true;
        let cache = PreparationCache::default();
        let original = Arc::new(
            Site::prepare(&config, &sources, store, directory.path(), info, &cache).unwrap(),
        );
        let catalogue = original.response("search-catalog.json").unwrap().unwrap();
        let index = format!("{}search-manifest.json", original.catalogue.base);
        let search = original.response(&index).unwrap().unwrap();
        std::fs::rename(
            directory.path().join("data"),
            directory.path().join("unavailable-archive"),
        )
        .unwrap();
        let renderer =
            Renderer::test_source("globalThis.aggrRender = () => 'updated template'; void 0;");
        let updated = Arc::new(
            original
                .reload_ui_with_renderer(&config, &cache, renderer)
                .unwrap()
                .unwrap(),
        );
        assert!(
            Arc::ptr_eq(&updated.prepared, &original.prepared),
            "UI reload never copies archived Markdown bodies"
        );
        assert_eq!(
            updated.ui.build_ctx.content_version,
            original.ui.build_ctx.content_version
        );
        assert_ne!(
            updated.ui.build_ctx.app_version,
            original.ui.build_ctx.app_version
        );
        assert_eq!(
            updated.response("index.html").unwrap().unwrap(),
            b"updated template"
        );
        assert_eq!(
            updated.response("search-catalog.json").unwrap().unwrap(),
            catalogue
        );
        assert_eq!(updated.response(&index).unwrap().unwrap(), search);
        assert!(
            !updated.root().join("pagefind").exists(),
            "UI changes reuse the existing immutable search generation"
        );
        assert!(Arc::ptr_eq(
            updated.search_source.as_ref().unwrap(),
            &original
        ));
        assert!(
            updated
                .reload_ui_with_renderer(&config, &cache, updated.ui.renderer.clone())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn pwa_shell_is_ready_without_archive_pages_and_catalogue_prepares_once() {
        let directory = tempfile::tempdir().unwrap();
        let (config, sources, store) =
            crate::site::tests::fixture(directory.path(), 3, "max_age_days = 30\npwa = true\n");
        let mut info = crate::site::tests::info(directory.path().join("lazy"));
        info.development = true;
        let site = Arc::new(
            Site::prepare(
                &config,
                &sources,
                store,
                directory.path(),
                info,
                &PreparationCache::default(),
            )
            .unwrap(),
        );
        assert!(site.response("sw.js").unwrap().is_some());
        assert!(site.response("manifest.webmanifest").unwrap().is_some());
        assert!(!site.root().join("items").exists());
        assert!(!site.root().join("pagefind").exists());
        assert!(!site.start_offline_catalogue().unwrap());
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !site.start_offline_catalogue().unwrap() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let bytes = site.response("offline-catalog.json").unwrap().unwrap();
        let catalogue: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(catalogue.as_array().unwrap().len(), 3);
        for item in catalogue.as_array().unwrap() {
            let url = item["url"].as_str().unwrap();
            let resource = item["resources"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["url"] == url)
                .unwrap();
            let key = format!("{url}index.html");
            assert_eq!(
                resource["revision"],
                crate::model::sha1_hex(site.response(&key).unwrap().unwrap())
            );
        }
        assert!(site.start_offline_catalogue().unwrap());
    }

    #[test]
    fn ready_resources_and_unrelated_routes_do_not_wait_for_a_cold_article() {
        let directory = tempfile::tempdir().unwrap();
        let site = site(directory.path());
        let ready = site.response("index.html").unwrap().unwrap();
        let key = "items/blog/2026-09-01-post-0/index.html";
        let index = site.articles["items/blog/2026-09-01-post-0/"];
        let blocked = lock(&site.materialized[index]);
        std::thread::scope(|scope| {
            let site = &site;
            let (started, started_rx) = mpsc::channel();
            let cold = scope.spawn(move || {
                started.send(()).unwrap();
                site.response(key).unwrap().unwrap()
            });
            started_rx.recv().unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !lock(&site.routes).contains_key("items/blog/2026-09-01-post-0/") {
                if std::time::Instant::now() >= deadline {
                    drop(blocked);
                    panic!("cold request never entered materialization");
                }
                std::thread::yield_now();
            }
            let (sent, received) = mpsc::channel();
            scope.spawn(move || {
                let cached = site.response("index.html").unwrap().unwrap();
                let unrelated = site
                    .response("items/blog/2026-09-02-post-1/index.html")
                    .unwrap()
                    .unwrap();
                sent.send((cached, unrelated)).unwrap();
            });
            let result = received.recv_timeout(Duration::from_secs(5));
            drop(blocked);
            assert_eq!(result.unwrap().0, ready);
            assert!(!cold.join().unwrap().is_empty());
        });
    }

    #[test]
    fn cold_work_budget_is_shared_by_generations_and_ready_files_bypass_it() {
        let directory = tempfile::tempdir().unwrap();
        let cache = PreparationCache {
            cold_work: Arc::new(ColdWork::new(2)),
            ..Default::default()
        };
        let old = site_with_cache(&directory.path().join("old"), &cache);
        let current = site_with_cache(&directory.path().join("current"), &cache);
        let expected = current.response("index.html").unwrap().unwrap();
        let permits: Vec<_> = (0..old.cold_work.capacity)
            .map(|_| old.cold_work.acquire())
            .collect();
        std::thread::scope(|scope| {
            let current = &current;
            let (cold_sent, cold_received) = mpsc::channel();
            scope.spawn(move || {
                cold_sent
                    .send(current.response("items/blog/2026-09-01-post-0/index.html"))
                    .unwrap();
            });
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !lock(&current.routes).contains_key("items/blog/2026-09-01-post-0/") {
                if std::time::Instant::now() >= deadline {
                    drop(permits);
                    panic!("cold request never entered materialization");
                }
                std::thread::yield_now();
            }
            let (ready_sent, ready_received) = mpsc::channel();
            scope.spawn(move || ready_sent.send(current.response("index.html")).unwrap());
            let ready = ready_received.recv_timeout(Duration::from_secs(5));
            let cold_waited = cold_received.try_recv().is_err();
            drop(permits);
            assert_eq!(ready.unwrap().unwrap().unwrap(), expected);
            assert!(
                cold_waited,
                "new generations share the old generation's CPU allowance"
            );
            assert!(
                cold_received
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .unwrap()
                    .is_some()
            );
        });
    }

    #[test]
    fn unfinished_files_are_not_served_as_published_resources() {
        let directory = tempfile::tempdir().unwrap();
        let site = site(directory.path());
        let key = "assets/images/unfinished.png";
        let path = site.root().join(key);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"partial bytes").unwrap();
        assert!(site.response(key).unwrap().is_none());
    }

    #[test]
    fn concurrent_article_representations_publish_one_complete_family() {
        let directory = tempfile::tempdir().unwrap();
        let site = site(directory.path());
        let path = "items/blog/2026-09-01-post-0";
        let keys: Vec<_> = ["/index.html", ".md", ".txt", ".rst", ".json"]
            .map(|suffix| format!("{path}{suffix}"))
            .into_iter()
            .collect();
        std::thread::scope(|scope| {
            let threads: Vec<_> = keys
                .iter()
                .map(|key| scope.spawn(|| site.response(key).unwrap().unwrap()))
                .collect();
            for (key, thread) in keys.iter().zip(threads) {
                let bytes = thread.join().unwrap();
                assert!(!bytes.is_empty());
                assert_eq!(bytes, std::fs::read(site.root().join(key)).unwrap());
                assert!(
                    keys.iter().all(|key| site.is_ready(key)),
                    "the whole family is visible together"
                );
            }
        });
    }
}
