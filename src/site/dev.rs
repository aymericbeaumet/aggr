//! Request-driven development rendering. The archive's display metadata is shared, while pages,
//! media and the search index are materialized only when a browser asks for them.

use std::collections::BTreeMap;
use std::path::{Component, Path};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use anyhow::Result;

use super::*;

#[derive(Default)]
pub(crate) struct PreparationCache {
    normalized: Mutex<BTreeMap<String, (String, Item)>>,
    markdown: Mutex<BTreeMap<String, Arc<content::PreparedMarkdown>>>,
    prepared: Mutex<Option<(String, PreparedSite)>>,
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
) -> Arc<content::PreparedMarkdown> {
    let key = cache.map(|_| crate::model::sha1_hex(body.as_bytes()));
    if let Some(value) = cache.and_then(|cache| {
        key.as_ref()
            .and_then(|key| lock(&cache.markdown).get(key).cloned())
    }) {
        return value;
    }
    let prepared = Arc::new(content::PreparedMarkdown::new(body));
    if let (Some(cache), Some(key)) = (cache, key) {
        lock(&cache.markdown).insert(key, prepared.clone());
    }
    prepared
}

struct Collection {
    kind: &'static str,
    title: String,
    prefix: String,
    indices: Vec<usize>,
    source: Option<usize>,
    category: Option<(bool, usize)>,
}

pub(crate) struct Site {
    prepared: PreparedSite,
    store: Store,
    derive_previews: bool,
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
    archive_value: minijinja::Value,
    search: Mutex<bool>,
    catalogue: pagefind::SearchCatalogue,
    // A render can create a page plus its media. Serialize materialization, so another request
    // never sees a half-written family; serving previously materialized files is inexpensive.
    materialized: Mutex<Materialized>,
    directory: Option<tempfile::TempDir>,
}

#[derive(Default)]
struct Materialized {
    assets: assets::Published,
    previews: BTreeMap<usize, Option<context::PreviewCtx>>,
    media: BTreeMap<usize, (ItemCtx, Vec<content::LocalImage>)>,
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
        prepared.renderer.freeze_templates()?;
        prepared.renderer.write_static(&info.out)?;
        write(&info.out.join(MARKER), env!("CARGO_PKG_VERSION").as_bytes())?;
        write(
            &info.out.join("updates.json"),
            outputs::updates(&prepared.site, &prepared.build_ctx)?.as_bytes(),
        )?;
        let articles: BTreeMap<_, _> = prepared
            .archive_items
            .iter()
            .enumerate()
            .map(|(i, item)| (item.url.clone(), i))
            .collect();
        let mut collections = vec![Collection {
            kind: "river",
            title: prepared.site.title.clone(),
            prefix: String::new(),
            indices: prepared
                .river_items
                .iter()
                .filter_map(|item| articles.get(&item.url).copied())
                .collect(),
            source: None,
            category: None,
        }];
        for (i, source) in prepared.source_ctxs.iter().enumerate() {
            collections.push(Collection {
                kind: "source",
                title: source.name.clone(),
                prefix: source.page.clone(),
                indices: prepared
                    .source_members
                    .get(&source.slug)
                    .cloned()
                    .unwrap_or_default(),
                source: Some(i),
                category: None,
            });
        }
        for (is_tag, terms, members) in [
            (false, &prepared.categories, &prepared.category_members),
            (true, &prepared.tags, &prepared.tag_members),
        ] {
            for (i, term) in terms.iter().enumerate() {
                collections.push(Collection {
                    kind: if is_tag { "tag" } else { "category" },
                    title: term.name.clone(),
                    prefix: term.page.clone(),
                    indices: members.get(&term.slug).cloned().unwrap_or_default(),
                    source: None,
                    category: Some((is_tag, i)),
                });
            }
        }
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
        log::info!(
            "dev: prepared {} article routes in {:.3}s; pages, media and search render on demand",
            articles.len(),
            started.elapsed().as_secs_f64()
        );
        let media_memo = assets::MediaMemo::new(store.image_cache().cloned());
        // Dev generations have private immutable index URLs. A catalogue can therefore name a
        // deferred index before Pagefind runs, without exposing mixed chunks during a reload.
        use sha2::{Digest as _, Sha256};
        let index_version = hex::encode(Sha256::digest(format!(
            "{}:{}",
            info.out.display(),
            prepared.build_ctx.content_version
        )));
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
            &prepared.build_ctx,
            &prepared.source_ctxs,
            &prepared.categories,
            &prepared.tags,
        );
        let archive_value = minijinja::Value::from_serialize(&prepared.archive_items);
        Ok(Self {
            prepared,
            store,
            derive_previews: config.fetch.previews,
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
            archive_value,
            search: Mutex::new(false),
            catalogue,
            materialized: Mutex::default(),
            directory: None,
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.info.out
    }

    pub(crate) fn own_directory(mut self, directory: tempfile::TempDir) -> Self {
        self.directory = Some(directory);
        self
    }

    fn pages(&self) -> Pages<'_> {
        let prepared = &self.prepared;
        Pages::prepared(
            &prepared.site,
            &prepared.renderer,
            self.shared.clone(),
            &prepared.archive_items,
            self.archive_value.clone(),
            prepared.per_page,
        )
    }

    fn with_preview(&self, index: usize, state: &mut Materialized) -> Result<ItemCtx> {
        let mut ctx = self.prepared.archive_items[index].clone();
        if self.shared_previews.contains(&index) {
            return Ok(ctx);
        }
        let preview = match state.previews.get(&index) {
            Some(preview) => preview.clone(),
            None => {
                let item = &self.prepared.all_items[index];
                let derive = self
                    .sources
                    .iter()
                    .find(|source| source.slug == item.front.source)
                    .map_or(self.derive_previews, |source| source.previews);
                let media =
                    assets::ItemMedia::gather_preview(&self.store, item, derive, &self.media_memo)?;
                let (preview, _, _) = media.publish(
                    self.root(),
                    &mut state.assets,
                    &mut budget::MediaBudget::new(u64::MAX),
                )?;
                state.previews.insert(index, preview.clone());
                preview
            }
        };
        ctx.preview = preview;
        Ok(ctx)
    }

    fn with_media(
        &self,
        index: usize,
        state: &mut Materialized,
    ) -> Result<(ItemCtx, Vec<content::LocalImage>)> {
        if let Some(media) = state.media.get(&index) {
            return Ok(media.clone());
        }
        let mut ctx = self.with_preview(index, state)?;
        let item = &self.prepared.all_items[index];
        let compact_cache = self
            .info
            .pagefind_cache
            .as_deref()
            .map(|root| crate::cache::Namespace::DeploymentMedia.dir(root));
        let media = assets::ItemMedia::gather(
            &self.store,
            item,
            false,
            &self.media_memo,
            !budget::full_quality(item.created_at(), self.info.now, self.full_quality_days),
            compact_cache.as_deref(),
        )?;
        let (_, images, document) = media.publish(
            self.root(),
            &mut state.assets,
            &mut budget::MediaBudget::new(u64::MAX),
        )?;
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
        state.media.insert(index, (ctx.clone(), images.clone()));
        Ok((ctx, images))
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
        if key.starts_with(&self.catalogue.base) {
            self.prepare_search()?;
            return match std::fs::read(self.root().join(key)) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.into()),
            };
        }
        let mut state = lock(&self.materialized);
        let path = self.root().join(key);
        if !path.is_file() && !self.materialize(key, &mut state)? {
            return Ok(None);
        }
        // Only generated routes and assets are reachable. Never expose store paths, config files
        // or renderer bookkeeping merely because they exist beside generated pages.
        if key.starts_with('.') {
            return Ok(None);
        }
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
        let documents = (0..self.prepared.archive_items.len())
            .map(|index| {
                let ctx = self.with_preview(index, &mut lock(&self.materialized))?;
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
        *built = true;
        Ok(())
    }

    fn materialize(&self, key: &str, state: &mut Materialized) -> Result<bool> {
        let prepared = &self.prepared;
        if let Some((collection, pager)) = self.lists.get(key) {
            let collection = &self.collections[*collection];
            let mut items: Vec<_> = collection
                .indices
                .iter()
                .map(|&i| prepared.archive_items[i].clone())
                .collect();
            for index in pager.range.clone() {
                items[index] = self.with_preview(collection.indices[index], state)?;
            }
            let category = collection.category.map(|(tag, i)| {
                if tag {
                    &prepared.tags[i]
                } else {
                    &prepared.categories[i]
                }
            });
            let page = ListPage {
                kind: collection.kind,
                title: &collection.title,
                prefix: &collection.prefix,
                list: &items,
                source: collection.source.map(|i| &prepared.source_ctxs[i]),
                category,
            };
            write(
                &self.root().join(key),
                self.pages().list(&page, pager)?.as_bytes(),
            )?;
            return Ok(true);
        }
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
            let (ctx, images) = self.with_media(index, state)?;
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
                &prepared.build_ctx,
                &self.store,
                &ctx,
                item,
                &prepared.prepared_bodies[index],
                &BTreeMap::from([(item.path.clone(), images)]),
                &shared,
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
        let simple = match key {
            "browse/index.html" => Some(("browse", "Browse", "browse/", "browse.html")),
            "sources/index.html" => Some(("sources", "Sources", "sources/", "browse.html")),
            "categories/index.html" => {
                Some(("categories", "Categories", "categories/", "browse.html"))
            }
            "tags/index.html" => Some(("tags", "Tags", "tags/", "browse.html")),
            "preferences/index.html" => Some((
                "preferences",
                "Preferences",
                "preferences/",
                "preferences.html",
            )),
            "404.html" => Some(("404", "Not found", "404.html", "404.html")),
            "offline.html" if self.prepared.site.pwa => {
                Some(("offline", "Offline", "offline.html", "offline.html"))
            }
            "manifest.webmanifest" if self.prepared.site.pwa => Some((
                "manifest",
                prepared.site.title.as_str(),
                "manifest.webmanifest",
                "manifest.webmanifest",
            )),
            _ => None,
        };
        if let Some((kind, title, path, template)) = simple {
            write(
                &self.root().join(key),
                self.pages()
                    .simple(SimplePage::new(kind, title, path, template))?
                    .as_bytes(),
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
                    .map(|&i| self.with_preview(i, state))
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
                    &prepared.build_ctx,
                    &collection.title,
                    &collection.prefix,
                    &items,
                )?;
                if collection.prefix.is_empty() {
                    write(
                        &self.root().join("feed.xml"),
                        outputs::atom_feed(&prepared.site, &prepared.build_ctx, &items).as_bytes(),
                    )?;
                }
                return Ok(true);
            }
        }
        let updated = prepared
            .archive_items
            .iter()
            .map(archive_modified_at)
            .max()
            .unwrap_or(prepared.build_ctx.time);
        let body = match key {
            "aggr.json" => {
                outputs::instance_descriptor(&prepared.site, &prepared.build_ctx, updated)?
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
        let cache = PreparationCache::default();
        for label in ["cold", "hot"] {
            let info = BuildInfo {
                out: temporary.path().join(label),
                base_url: None,
                config_sha: None,
                config_path: Some("aggr.toml".into()),
                data_sha: None,
                generation: generation.clone(),
                now,
                release: false,
                discussions: crate::discussions::ResolutionSet::default(),
                development: true,
                render_cache_key: None,
                pagefind_cache: Some(image_cache.clone()),
            };
            let started = Instant::now();
            let site = Site::prepare(
                &config,
                &sources,
                Store::open(Path::new(&data)).with_image_cache(&image_cache),
                root,
                info,
                &cache,
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
