//! Shared fetch stage for sync/build/dev: every source runs in parallel and writes only its own
//! directory. Source-local keys and shared URL reservations decide what is new; no git operations.

mod archive;
mod documents;
mod duration;
mod index;
mod links;
mod openreview;
mod plan;
mod podcast;
mod recording;
mod repair;
#[cfg(test)]
pub(crate) mod tests;
mod transaction;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use chrono::{DateTime, Utc};
use futures_util::{StreamExt as _, stream};
use tokio::sync::{OnceCell, Semaphore};
use tokio::task::JoinSet;

use self::index::{ExistingPaths, index_archive};
use self::links::{LinkTransaction, SharedLinks};
use self::plan::{
    apply_first_import_limit, apply_validators, merge_image_metadata, persist_item, prepare_item,
    redact_source_secrets, should_persist_state, source_metadata_changed, source_request_state,
    use_existing_path,
};
use self::repair::{
    reconcile_duration, reconcile_podcast, repair_archived_images, repair_feed_captures,
    repair_recordings, reprocess_stored_bodies,
};
use self::transaction::SourceTransaction;
use super::Project;
use crate::cli::FetchArgs;
use crate::config::{ContentMode, Engine, Source, StoreConfig};
use crate::content;
use crate::git::Worktree;
use crate::http;
use crate::media;
use crate::model::{
    ContentKind, RawItem, dedupe_keys, file_stem, item_dir, normalize_link, unique_stem,
};
use crate::preview;
use crate::sources::{self, Fetch};
use crate::store::{Outcome, Store, retention};

pub struct Report {
    pub sources: Vec<SourceReport>,
    pub status_changed: bool,
    /// Items deleted by `[store]` retention.
    pub removed: usize,
    /// Retained bodies re-derived locally, independently of configured source outcomes.
    pub reprocessed: usize,
}

pub struct SourceReport {
    pub slug: String,
    pub outcome: Outcome,
    pub added: usize,
    pub unchanged: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StatePolicy {
    /// Preserve the no-new-items/no-commit invariant of the append-only branch.
    PersistentBranch,
    /// Keep validators current in dev's private cache even when no item was added.
    DevCache,
}

impl Report {
    pub fn added(&self) -> usize {
        self.sources.iter().map(|s| s.added).sum::<usize>() + self.reprocessed
    }

    pub fn ok(&self) -> usize {
        self.sources
            .iter()
            .filter(|s| s.outcome == Outcome::Ok)
            .count()
    }

    pub fn errors(&self) -> usize {
        self.sources.len() - self.ok()
    }

    pub fn all_failed(&self) -> bool {
        !self.sources.is_empty() && self.ok() == 0
    }
}

/// Articles converted or written at once per process. Both stages are CPU-bound and run on the
/// blocking pool, so the bound is the parallelism the machine has rather than a fixed pair of
/// slots. `sync` and `build` own the machine while they fetch, and a floor of two keeps the
/// pipeline overlapping on a single-core runner. `dev` fetches in the background while it
/// rebuilds, so it takes half and leaves the rest to the build the reader is waiting for.
fn preparation_slots(policy: StatePolicy) -> usize {
    let workers = crate::site::parallel::workers();
    match policy {
        StatePolicy::PersistentBranch => workers.max(2),
        StatePolicy::DevCache => workers.div_ceil(2).max(2),
    }
}
/// Recording-metadata probes in flight per process: network-bound, bounded for politeness.
const RECORDING_PROBE_SLOTS: usize = 8;

#[derive(Clone)]
struct Options {
    archived_links: Arc<SharedLinks>,
    existing_paths: Arc<OnceCell<ExistingPaths>>,
    dry_run: bool,
    refresh: bool,
    html: bool,
    html_max_bytes: usize,
    article_concurrency: usize,
    /// CPU slots for turning a fetched article into its stored form, off the runtime workers.
    preparation_limit: Arc<Semaphore>,
    /// Slots for writing an item, which decodes every image master and rendition to validate it.
    persist_limit: Arc<Semaphore>,
    /// Concurrent recording-metadata probes. Each may sit in an 8 s HTTP request, so they never
    /// borrow a CPU slot from article preparation.
    recording_limit: Arc<Semaphore>,
    max_items_per_source: usize,
    preview_fetcher: Arc<preview::Fetcher>,
    media_fetcher: Arc<media::Fetcher>,
    now: DateTime<Utc>,
}

pub async fn run(project: &Project, worktree: &Worktree, args: &FetchArgs) -> Result<Report> {
    let cache_dir = project.build_cache_dir()?;
    run_with_cache(
        project,
        worktree,
        args,
        &cache_dir,
        StatePolicy::PersistentBranch,
    )
    .await
}

pub async fn run_with_cache(
    project: &Project,
    worktree: &Worktree,
    args: &FetchArgs,
    cache_dir: &Path,
    state_policy: StatePolicy,
) -> Result<Report> {
    let selected: Vec<Source> = project.sources.clone();
    let store = Arc::new(Store::open(worktree.dir()).with_image_cache(cache_dir));
    let client = Arc::new(http::Client::new(&project.config.fetch)?);
    let index_store = store.clone();
    let reprocess_args = args.clone();
    let store_root = worktree.dir().to_path_buf();
    let (known_links, existing_paths, reprocessed) = tokio::task::spawn_blocking(move || {
        let reprocessed = reprocess_stored_bodies(&index_store, &reprocess_args, &store_root)?;
        let (links, paths) = index_archive(index_store.items()?);
        Ok::<_, anyhow::Error>((links, paths, reprocessed))
    })
    .await
    .context("preparing archived article bodies and index")??;
    if reprocessed > 0 {
        println!("reprocess: {reprocessed} stored article body(ies)");
    }
    let options = Options {
        archived_links: Arc::new(SharedLinks::new(known_links)),
        existing_paths: Arc::new(OnceCell::new_with(Some(existing_paths))),
        dry_run: args.dry_run,
        refresh: args.refresh,
        html: project.config.store.html,
        html_max_bytes: project.config.store.html_max_bytes,
        article_concurrency: project.config.fetch.article_concurrency,
        preparation_limit: Arc::new(Semaphore::new(preparation_slots(state_policy))),
        persist_limit: Arc::new(Semaphore::new(preparation_slots(state_policy))),
        recording_limit: Arc::new(Semaphore::new(RECORDING_PROBE_SLOTS)),
        max_items_per_source: project.config.fetch.max_items_per_source,
        preview_fetcher: Arc::new(preview::Fetcher::new()?),
        media_fetcher: Arc::new(
            media::Fetcher::new(&project.config.fetch, media::MediaLimits::default())?
                .with_cache(cache_dir),
        ),
        now: Utc::now(),
    };
    let limit = Arc::new(Semaphore::new(project.config.fetch.concurrency));

    let mut tasks = JoinSet::new();
    for source in selected {
        let (store, store_root, client, cache_dir, options, limit) = (
            store.clone(),
            worktree.dir().to_path_buf(),
            client.clone(),
            cache_dir.to_path_buf(),
            options.clone(),
            limit.clone(),
        );
        tasks.spawn(async move {
            let started = Instant::now();
            let _permit = limit.acquire_owned().await;
            let article_failures = ArticleFailures::default();
            let result = fetch_one(
                &source,
                FetchOneContext {
                    store: &store,
                    store_root: &store_root,
                    client: &client,
                    cache_dir: &cache_dir,
                    options: &options,
                    article_failures: &article_failures,
                    state_policy,
                },
            )
            .await
            .map_err(|err| sanitized_source_error(&source, &err));
            (source.slug, result, started.elapsed())
        });
    }

    let mut reports = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        let (slug, result, elapsed) = joined.context("a fetch task panicked")?;
        let report = match result {
            Ok(report) => report,
            Err(err) => {
                log::warn!("{slug}: {err:#}");
                SourceReport {
                    slug,
                    outcome: Outcome::Error(format!("{err:#}")),
                    added: 0,
                    unchanged: false,
                }
            }
        };
        println!(
            "{}",
            source_progress(&report, elapsed, reports.len() + 1, project.sources.len())
        );
        reports.push(report);
    }
    let order: BTreeMap<&str, usize> = project
        .sources
        .iter()
        .enumerate()
        .map(|(i, s)| (s.slug.as_str(), i))
        .collect();
    reports.sort_by_key(|report| order.get(report.slug.as_str()).copied());

    let outcomes: BTreeMap<String, Outcome> = reports
        .iter()
        .map(|r| (r.slug.clone(), r.outcome.clone()))
        .collect();
    let known: BTreeSet<String> = project.sources.iter().map(|s| s.slug.clone()).collect();
    let mut status = store.status()?;
    let status_changed = status.apply(&outcomes, &known, options.now);
    if status_changed && !args.dry_run {
        store.write_status(&status)?;
    }

    let removed = if args.dry_run {
        0
    } else {
        apply_retention(&store, &project.config.store, options.now)?
    };
    if removed > 0 {
        println!("retention: -{removed}");
    }
    Ok(Report {
        sources: reports,
        status_changed,
        removed,
        reprocessed,
    })
}

fn source_progress(
    report: &SourceReport,
    elapsed: Duration,
    completed: usize,
    total: usize,
) -> String {
    let outcome = match &report.outcome {
        Outcome::Error(message) => format!("error: {message}"),
        Outcome::Ok if report.unchanged => "unchanged".into(),
        Outcome::Ok => format!("+{}", report.added),
    };
    format!(
        "{}: {outcome} ({:.1}s; {completed}/{total} sources complete)",
        report.slug,
        elapsed.as_secs_f64()
    )
}

fn sanitized_source_error(source: &Source, err: &anyhow::Error) -> anyhow::Error {
    anyhow::anyhow!(redact_source_secrets(source, &format!("{err:#}")))
}

/// Drop what `[store] max_age_days` / `max_items` exclude. A no-op unless one of them is set.
fn apply_retention(store: &Store, config: &StoreConfig, now: DateTime<Utc>) -> Result<usize> {
    let limits = retention::Limits {
        max_age_days: config.max_age_days,
        max_items: config.max_items,
    };
    if limits.is_unbounded() {
        return Ok(0);
    }
    let drop = retention::plan(&store.items()?, limits, now);
    for path in &drop {
        store.remove_item(path)?;
    }
    Ok(drop.len())
}

#[derive(Clone, Copy)]
struct FetchOneContext<'a> {
    store: &'a Arc<Store>,
    store_root: &'a Path,
    client: &'a http::Client,
    cache_dir: &'a Path,
    options: &'a Options,
    article_failures: &'a ArticleFailures,
    state_policy: StatePolicy,
}

async fn fetch_one(source: &Source, context: FetchOneContext<'_>) -> Result<SourceReport> {
    let mut links = LinkTransaction::default();
    let mut transaction = (!context.options.dry_run)
        .then(|| SourceTransaction::new(context.store_root))
        .transpose()?;
    let result = fetch_one_inner(source, context, transaction.as_mut(), &mut links).await;
    match result {
        Ok(report) => {
            if let Some(transaction) = transaction {
                transaction.commit();
            }
            links.commit();
            Ok(report)
        }
        Err(error) => {
            if let Some(mut transaction) = transaction
                && let Err(rollback) = transaction.rollback()
            {
                return Err(error.context(format!(
                    "rolling back failed source transaction: {rollback:#}"
                )));
            }
            Err(error)
        }
    }
}

async fn fetch_one_inner(
    source: &Source,
    context: FetchOneContext<'_>,
    mut transaction: Option<&mut SourceTransaction>,
    links: &mut LinkTransaction,
) -> Result<SourceReport> {
    let FetchOneContext {
        store,
        store_root,
        client,
        cache_dir,
        options,
        article_failures,
        state_policy,
        ..
    } = context;
    let slug = &source.slug;
    let state = store.source_state(slug)?;
    let mut request_state = source_request_state(&state, options.refresh);
    let parser_receipt = duration::prepare(source, &mut request_state, cache_dir)?;
    let ctx = sources::Context {
        client,
        state: &request_state,
        cache_dir,
    };
    let fetched = sources::fetch(source, &ctx).await?;

    let mut next_state = state.clone();
    next_state.identity = source.identity.clone();
    let (mut report, metadata_changed) = match fetched {
        Fetch::Unchanged { validators } => {
            apply_validators(validators, &mut next_state, source);
            (
                SourceReport {
                    slug: slug.clone(),
                    outcome: Outcome::Ok,
                    added: 0,
                    unchanged: true,
                },
                false,
            )
        }
        Fetch::Changed {
            validators,
            meta,
            mut items,
        } => {
            apply_validators(validators, &mut next_state, source);
            next_state.title = meta.title.or(next_state.title);
            next_state.site_url = meta
                .site_url
                .and_then(|url| url::Url::parse(&url).ok())
                .map(|url| crate::config::public_url(&url, true))
                .or(next_state.site_url);
            next_state.language = meta.language.or(next_state.language);
            let metadata_changed = source_metadata_changed(&state, &next_state);

            let seen = store.seen(slug)?;
            let archive = options
                .existing_paths
                .get_or_try_init(|| async {
                    let root = store_root.to_path_buf();
                    tokio::task::spawn_blocking(move || {
                        Store::open(root)
                            .items()
                            .map(|items| index_archive(items).1)
                    })
                    .await
                    .context("indexing archived item paths")?
                })
                .await?;
            let existing_paths = archive.items.get(slug);
            let mut new_keys: Vec<String> = Vec::new();
            let mut prospective = seen.clone();
            let mut taken: HashSet<(String, String)> = HashSet::new();
            let mut added = 0;
            apply_first_import_limit(&mut items, &source.engine, options.max_items_per_source);
            if podcast::is_source(source) {
                let mut counts = BTreeMap::new();
                for raw in &items {
                    if let Some(key) = podcast::identity(&raw.title, raw.published) {
                        *counts.entry(key).or_insert(0usize) += 1;
                    }
                }
                let mut remaining = Vec::with_capacity(items.len());
                for raw in items {
                    let unique = podcast::identity(&raw.title, raw.published)
                        .is_some_and(|key| counts.get(&key) == Some(&1));
                    let path = unique.then(|| archive.podcasts.path(slug, &raw)).flatten();
                    let reconciled = if let Some(path) = path {
                        reconcile_podcast(
                            &raw,
                            path,
                            source,
                            store,
                            options,
                            transaction.as_deref_mut(),
                            &prospective,
                        )?
                    } else {
                        None
                    };
                    if let Some((changed, aliases)) = reconciled {
                        added += usize::from(changed);
                        prospective.extend(aliases.iter().cloned());
                        new_keys.extend(aliases);
                    } else {
                        remaining.push(raw);
                    }
                }
                items = remaining;
            }
            // Assign paths in feed order before downloading so completion order cannot change
            // collision suffixes or make a slow article hold up later downloads.
            let mut candidates = Vec::new();
            {
                let archived = options.archived_links.state();
                for raw in items.into_iter().rev() {
                    let keys = dedupe_keys(&raw);
                    let known = keys.iter().any(|key| prospective.contains(key));
                    if known && !options.refresh {
                        if let Some(path) = existing_paths
                            .and_then(|paths| keys.iter().find_map(|key| paths.get(key)))
                        {
                            added += usize::from(reconcile_duration(
                                &raw,
                                path,
                                source,
                                store,
                                options,
                                transaction.as_deref_mut(),
                            )?);
                        }
                        continue;
                    }
                    if !known && archived.committed.contains(&normalize_link(&raw.link)) {
                        continue;
                    }
                    if !known {
                        prospective.extend(keys.iter().cloned());
                    }
                    let existing_path = known
                        .then(|| {
                            keys.iter()
                                .find_map(|key| existing_paths.and_then(|paths| paths.get(key)))
                                .cloned()
                        })
                        .flatten();
                    if known && options.refresh && existing_path.is_none() {
                        log::debug!(
                            "skipping refresh for retained/deleted or unidentifiable item: {}",
                            raw.link
                        );
                        continue;
                    }
                    candidates.push((raw, keys, known, existing_path));
                }
            }
            // Reserve the whole source together to avoid cycles between overlapping feeds.
            // Disjoint sources keep downloading concurrently, with bounded article buffers.
            let wanted = candidates
                .iter()
                .filter(|(_, _, known, _)| !known)
                .map(|(raw, ..)| normalize_link(&raw.link))
                .filter(|link| !link.is_empty())
                .collect();
            links.reserve(&options.archived_links, wanted).await;
            candidates.retain(|(raw, _, known, _)| {
                *known || !links.duplicate(&normalize_link(&raw.link))
            });
            let candidates = candidates
                .into_iter()
                .map(|(raw, keys, known, existing_path)| {
                    let date = raw.created_at().unwrap_or(options.now).min(options.now);
                    let dir = item_dir(slug, date);
                    let stem = file_stem(date, &raw.title);
                    let stem = if existing_path.is_none() && !options.refresh {
                        let identity = keys.first().map(String::as_str).unwrap_or(&raw.link);
                        unique_stem(&stem, identity, |stem| {
                            store.stem_exists(&dir, stem)
                                || taken.contains(&(dir.clone(), stem.to_string()))
                        })
                    } else {
                        stem
                    };
                    taken.insert((dir, stem.clone()));
                    (raw, keys, known, existing_path, stem)
                })
                .collect::<Vec<_>>();
            let document_context = &context;
            let mut enriched = stream::iter(candidates)
                .map(|(raw, keys, known, existing_path, stem)| async move {
                    let mut raw = hydrate_new_mirror_companions(
                        raw,
                        known,
                        options.dry_run,
                        source.previews,
                        source.images.archives(),
                    )
                    .await;
                    if source.previews
                        && let Ok(url) = url::Url::parse(&raw.link)
                        && let Some(thumbnail) = sources::youtube::thumbnail(&url)
                        && !raw
                            .preview_candidates
                            .iter()
                            .any(|candidate| candidate.url == thumbnail)
                    {
                        raw.preview_candidates.push(preview::Candidate {
                            url: thumbnail,
                            alt: Some(raw.title.clone()),
                        });
                    }
                    let (mut raw, kind) =
                        heavy_content(&raw, source, client, cache_dir, article_failures).await;
                    if source.content == ContentMode::Light
                        && !matches!(source.engine, Engine::Aggr { .. })
                        && let Some(seconds) =
                            recording::infer(&raw, source, client, cache_dir, article_failures)
                                .await?
                    {
                        raw.extra.insert("duration_seconds".into(), seconds.into());
                    }
                    if known {
                        raw.preview = None;
                        raw.images.clear();
                    }
                    if !matches!(source.engine, Engine::Aggr { .. }) {
                        let stored = existing_path
                            .as_deref()
                            .map(|path| store.read_item(path))
                            .transpose()?;
                        if !options.dry_run
                            && stored
                                .as_ref()
                                .map(|item| {
                                    store.read_document(item).map(|asset| {
                                        asset.filter(|asset| {
                                            documents::url(&raw.link, &raw.extra)
                                                .is_some_and(|url| url.as_str() == asset.source_url)
                                        })
                                    })
                                })
                                .transpose()?
                                .flatten()
                                .is_none()
                            && let Some(url) = documents::url(&raw.link, &raw.extra)
                        {
                            raw.document = documents::capture(&url, source, document_context).await;
                        }
                        let has_stored_images = stored
                            .as_ref()
                            .is_some_and(|item| !item.front.images.is_empty());
                        if source.images.archives()
                            && (!known || options.refresh && !has_stored_images)
                            && raw.images.is_empty()
                            && let Ok(base) = url::Url::parse(&raw.link)
                            && let Some(poster) = options
                                .media_fetcher
                                .fetch_first(&sources::youtube::poster_candidates(&base), source)
                                .await
                        {
                            raw.images.push(poster);
                        }
                        if source.images.archives()
                            && let Some(html) = raw.content_html.as_deref()
                            && let Ok(base) = url::Url::parse(&raw.link)
                        {
                            let limits = media::MediaLimits::default();
                            let stored_count =
                                stored.as_ref().map_or(0, |item| item.front.images.len());
                            let available_slots = limits
                                .max_assets
                                .saturating_sub(stored_count + raw.images.len());
                            let candidates: Vec<_> =
                                media::article_candidates(html, &raw.preview_candidates, &base)
                                    .into_iter()
                                    .filter(|candidate| {
                                        !raw.images
                                            .iter()
                                            .any(|image| image.source_url == candidate.url.as_str())
                                            && !stored.as_ref().is_some_and(|item| {
                                                item.front.images.iter().any(|image| {
                                                    image.source == candidate.url.as_str()
                                                        && store
                                                            .image_files_present(&item.path, image)
                                                })
                                            })
                                    })
                                    .take(available_slots)
                                    .collect();
                            let retained = stored.as_ref().map_or(0, |item| {
                                item.front
                                    .images
                                    .iter()
                                    .map(|image| store.image_bytes_present(&item.path, image))
                                    .fold(0usize, usize::saturating_add)
                            });
                            let newly_retained = raw
                                .images
                                .iter()
                                .map(|image| {
                                    image.master_bytes.len()
                                        + image
                                            .renditions
                                            .iter()
                                            .map(|variant| variant.bytes.len())
                                            .sum::<usize>()
                                })
                                .sum::<usize>();
                            raw.images.extend(
                                options
                                    .media_fetcher
                                    .fetch_with_budget(
                                        &candidates,
                                        source,
                                        limits.max_article_bytes.saturating_sub(
                                            retained.saturating_add(newly_retained),
                                        ),
                                    )
                                    .await,
                            );
                        }
                        let has_stored_preview = if let Some(item) = stored.as_ref() {
                            store.read_preview(item)?.is_some()
                        } else {
                            false
                        };
                        if source.previews
                            && (!known || options.refresh && !has_stored_preview)
                            && raw.preview.is_none()
                            && let Ok(base) = url::Url::parse(&raw.link)
                        {
                            let candidates = preview::candidates(
                                &raw.preview_candidates,
                                raw.content_html.as_deref(),
                                &base,
                            );
                            raw.preview = options
                                .preview_fetcher
                                .fetch_with_assets(&candidates, source, &raw.images)
                                .await;
                            if raw.preview.is_none() {
                                let document = raw
                                    .extra
                                    .get("document_url")
                                    .and_then(serde_yaml_ng::Value::as_str)
                                    .and_then(|value| url::Url::parse(value).ok())
                                    .filter(preview::is_pdf_url)
                                    .unwrap_or(base);
                                raw.preview = options
                                    .preview_fetcher
                                    .fetch_document(&document, source)
                                    .await;
                            }
                        }
                    }
                    let (raw, mut planned) = prepare_item(raw, source, options, kind).await?;
                    planned.stem = stem;
                    Ok::<_, anyhow::Error>((raw, planned, keys, known, existing_path))
                })
                .buffer_unordered(options.article_concurrency);
            while let Some(result) = enriched.next().await {
                let (raw, mut planned, keys, known, existing_path) = result?;
                if let Some(existing_path) = existing_path.as_deref() {
                    use_existing_path(&mut planned, existing_path)?;
                }
                if planned.html.is_some() {
                    planned.front.html = Some(format!("{}.html", planned.stem));
                }
                if let Some(preview) = &raw.preview {
                    planned.front.preview = Some(preview.metadata(&planned.stem));
                } else if known {
                    let path = format!("{}/{}", planned.dir, planned.stem);
                    if let Ok(existing) = store.read_item(&path)
                        && store.read_preview(&existing)?.is_some()
                    {
                        planned.front.preview = existing.front.preview;
                    }
                }
                if !raw.images.is_empty() {
                    if known {
                        let path = format!("{}/{}", planned.dir, planned.stem);
                        planned.front.images = store.read_item(&path)?.front.images;
                    }
                    merge_image_metadata(&mut planned.front.images, &raw.images, &planned.stem);
                } else if known {
                    let path = format!("{}/{}", planned.dir, planned.stem);
                    if let Ok(existing) = store.read_item(&path) {
                        planned.front.images = store
                            .read_images(&existing)?
                            .into_iter()
                            .map(|image| image.metadata)
                            .collect();
                    }
                }
                if !options.dry_run {
                    if let Some(transaction) = transaction.as_mut() {
                        transaction.track_item(&planned, &raw)?;
                    }
                    persist_item(store, options, planned, raw).await?;
                }
                if !known {
                    new_keys.extend(keys);
                }
                added += 1;
            }
            if !options.dry_run && !new_keys.is_empty() {
                if let Some(transaction) = transaction.as_mut() {
                    transaction.track_seen(slug)?;
                }
                store.append_seen(slug, &new_keys, options.now)?;
            }
            (
                SourceReport {
                    slug: slug.clone(),
                    outcome: Outcome::Ok,
                    added,
                    unchanged: added == 0 && !metadata_changed,
                },
                metadata_changed,
            )
        }
    };
    let added = report.added;
    let duration_repairs = repair_recordings(source, &context, transaction.as_deref_mut()).await?;
    let repaired = documents::repair(source, &context, transaction.as_deref_mut()).await?
        + duration_repairs
        + repair_feed_captures(source, &context, transaction.as_deref_mut()).await?
        + repair_archived_images(source, store, options, transaction.as_deref_mut()).await?;
    report.added += repaired;
    report.unchanged &= repaired == 0;
    if !options.dry_run && should_persist_state(added, repaired, metadata_changed, state_policy) {
        if let Some(transaction) = transaction {
            transaction.track_state(slug)?;
        }
        store.write_source_state(slug, &next_state)?;
    }
    if !options.dry_run
        && let Some(receipt) = parser_receipt
    {
        receipt.commit()?;
    }
    Ok(report)
}

#[derive(Hash, PartialEq, Eq)]
enum ArticleFailureScope {
    Page(String),
    Origin(String),
}

#[derive(Default)]
struct ArticleFailures {
    blocked: Mutex<HashSet<ArticleFailureScope>>,
}

impl ArticleFailures {
    fn blocked(&self, url: &url::Url) -> bool {
        let blocked = self
            .blocked
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        blocked.contains(&ArticleFailureScope::Page(url.to_string()))
            || blocked.contains(&ArticleFailureScope::Origin(
                url.origin().ascii_serialization(),
            ))
    }

    fn record(&self, url: &url::Url, status: Option<u16>) {
        let scope = match status {
            Some(401 | 403) => ArticleFailureScope::Page(url.to_string()),
            Some(429) => ArticleFailureScope::Origin(url.origin().ascii_serialization()),
            _ => return,
        };
        self.blocked
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(scope);
    }
}

/// Whether a link points at a document that is final as feed content: a PDF or an image file,
/// which heavy extraction never requests.
fn is_binary_link(url: &url::Url) -> bool {
    preview::is_pdf_url(url)
        || url.path().rsplit_once('.').is_some_and(|(_, extension)| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "webp" | "gif" | "avif"
            )
        })
}

/// The account that published a page whose own URL does not name one. A YouTube watch URL is the
/// only such link aggr archives: every other platform puts the account in the path, where
/// [`crate::platform`] already reads it without asking the page.
fn page_publisher(page: &str, url: &url::Url) -> Option<String> {
    if crate::platform::account_path(url).is_some() || !sources::youtube::is_video_url(url) {
        return None;
    }
    sources::youtube::owner_profile(page, url)
}

fn recording_duration(
    page: &str,
    document: &scraper::Html,
    url: &url::Url,
    audio: Option<&str>,
) -> Option<u64> {
    if sources::youtube::is_video_url(url) {
        return sources::youtube::duration_seconds(page, url);
    }
    if !recording::is_recording(url.as_str(), audio) {
        return None;
    }
    let media: Vec<_> = audio
        .and_then(|value| url::Url::parse(value).ok())
        .into_iter()
        .collect();
    match crate::media_duration::from_document(document, url, &media) {
        Some(crate::media_duration::RecordingDuration::Recorded(seconds)) => Some(seconds),
        _ => None,
    }
}

/// Everything `heavy_content` reads from the original page. One blocking step produces it from a
/// single parsed document, so the runtime never parses HTML inline and no page is parsed twice.
struct PageAnalysis {
    /// The decoded page, handed on to extraction.
    page: String,
    duration: Option<u64>,
    /// The account that published the page, when its URL does not already name one.
    publisher: Option<String>,
    interactive: bool,
    candidates: preview::HtmlCandidateGroups,
    /// ActivityPub representations worth a discovery request; empty for ordinary articles.
    activity_alternates: Vec<url::Url>,
}

/// `page_url` is where the page was finally served from; `requested` is the item's own URL, which
/// YouTube duration extraction reads the video id from.
fn analyze_page(
    page: String,
    page_url: &url::Url,
    requested: &url::Url,
    audio: Option<&str>,
    wants_candidates: bool,
) -> PageAnalysis {
    // Lazy and responsive image sources are promoted into `src` before parsing, as the preview
    // candidate scan expects; the other checks read elements that pass leaves untouched.
    let normalized = content::normalize_image_sources(&page);
    let document = scraper::Html::parse_document(&normalized);
    let media_url = if sources::youtube::is_video_url(requested) {
        requested
    } else {
        page_url
    };
    let duration = recording_duration(&page, &document, media_url, audio);
    let publisher = page_publisher(&page, media_url);
    let interactive = crate::site::interactive::mentions_interactive_markup(&page)
        && crate::site::interactive::is_interactive_document_in(&document);
    let candidates = if wants_candidates {
        preview::html_candidate_groups_in(&document, page_url)
    } else {
        preview::HtmlCandidateGroups::default()
    };
    let activity_alternates = if crate::threads::may_advertise_activity(&page, page_url) {
        crate::threads::activity_candidates_in(&document, page_url)
    } else {
        Vec::new()
    };
    PageAnalysis {
        page,
        duration,
        publisher,
        interactive,
        candidates,
        activity_alternates,
    }
}

/// The article a script shell ships in one of its own modules: fetched as text, never run, and
/// read back into HTML that then goes through the ordinary extraction and storage pipeline.
async fn article_from_modules(
    modules: &[url::Url],
    raw: &RawItem,
    source: &Source,
    client: &http::Client,
    final_url: &url::Url,
) -> Result<content::ExtractedArticle> {
    let slug = final_url
        .path_segments()
        .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
        .unwrap_or_default()
        .to_string();
    let mut best: Option<String> = None;
    for module in modules {
        let headers = http::source_headers(source, module);
        let body = match client
            .get(http::Request {
                url: module,
                headers,
                etag: None,
                last_modified: None,
            })
            .await
        {
            Ok(http::Response::Ok(body)) => body,
            Ok(http::Response::NotModified) => continue,
            Err(error) => {
                log::debug!("{}: module {module} unavailable: {error:#}", source.slug);
                continue;
            }
        };
        let script = body
            .content_type
            .as_deref()
            .is_none_or(|kind| kind.contains("javascript") || kind.contains("ecmascript"))
            || module.path().ends_with(".js")
            || module.path().ends_with(".mjs");
        if !script {
            continue;
        }
        let text = String::from_utf8_lossy(&body.bytes);
        if let Some(html) = content::article_from_module(&text, &slug)
            && best
                .as_ref()
                .is_none_or(|current| current.len() < html.len())
        {
            best = Some(html);
        }
    }
    let html = best.context("no module script carries the article")?;
    log::info!(
        "{}: read the article for {final_url} from the page's own script module",
        source.slug
    );
    let document = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title></head><body><article>{html}</article></body></html>",
        content::escape_html(&raw.title)
    );
    content::extract_article_async(document, final_url.clone()).await
}

async fn heavy_content(
    raw: &RawItem,
    source: &Source,
    client: &http::Client,
    cache_dir: &Path,
    failures: &ArticleFailures,
) -> (RawItem, ContentKind) {
    let fallback = if raw.content_html.is_some() {
        ContentKind::Feed
    } else {
        ContentKind::None
    };
    if source.content == ContentMode::Light
        || matches!(source.engine, Engine::Aggr { .. })
        || (raw.content_html.is_some()
            && source.engine.url().is_some_and(sources::qwen::is_blog_url))
    {
        return (raw.clone(), fallback);
    }
    let Ok(url) = url::Url::parse(&raw.link) else {
        return (raw.clone(), fallback);
    };
    if is_binary_link(&url) {
        return (raw.clone(), fallback);
    }
    let known_subscription = raw
        .extra
        .get("subscription_required")
        .and_then(serde_yaml_ng::Value::as_bool)
        == Some(true);
    if failures.blocked(&url) && !known_subscription {
        return (raw.clone(), fallback);
    }
    if let Some(id) = openreview::note_id(&url) {
        let cache = crate::cache::ArticleCache::new(cache_dir);
        return match openreview::enrich(raw, &id, source, client, &cache).await {
            Ok(enriched) => (enriched, ContentKind::Extracted),
            Err(error) => {
                log::debug!(
                    "{}: OpenReview paper unavailable for {}: {error:#}",
                    source.slug,
                    raw.link
                );
                (raw.clone(), fallback)
            }
        };
    }
    if let Some(id) = sources::qwen::post_id(&url) {
        // The post page is a script shell; the article API the Qwen source reads lists it.
        match sources::qwen::article(&url, &id, source, client).await {
            Ok(Some(post)) => {
                let mut enriched = raw.clone();
                enriched.content_html = post.content_html;
                if source.previews || source.images.archives() {
                    enriched.preview_candidates = post.preview_candidates;
                }
                return (enriched, ContentKind::Extracted);
            }
            Ok(None) => log::debug!("{}: Qwen article API does not list {id}", source.slug),
            Err(error) => log::debug!("{}: Qwen article API unavailable: {error:#}", source.slug),
        }
    }
    let mut preview_candidates = raw.preview_candidates.clone();
    let mut page_candidates = preview::HtmlCandidateGroups::default();
    let mut interactive = false;
    let mut duration = None;
    let mut publisher = None;
    let mut thread_link = None;
    let mut subscription_required = false;
    let mut archive_provenance = None;
    let result = async {
        if failures.blocked(&url) { anyhow::bail!("original page is already unavailable for this run"); }
        let cache = crate::cache::ArticleCache::new(cache_dir);
        match crate::threads::expand_x(&url, source, client, &cache).await {
            Ok(Some(expanded)) => {
                thread_link = crate::threads::canonical_x_url(&url);
                preview_candidates.clear();
                page_candidates = preview::html_candidate_groups(&expanded.html, &url);
                return Ok(Some(expanded));
            }
            Ok(None) => {}
            Err(error) => log::debug!(
                "{}: X thread expansion failed for {}: {error:#}",
                source.slug,
                url
            ),
        }
        let headers = http::source_headers(source, &url);
        let cached = cache.load(&url, headers)?;
        let request = client.get(http::Request {
            url: &url,
            headers,
            etag: cached.as_ref().and_then(|entry| entry.etag.as_deref()),
            last_modified: cached
                .as_ref()
                .and_then(|entry| entry.last_modified.as_deref()),
        });
        let response = if sources::youtube::is_video_url(&url) {
            tokio::time::timeout(std::time::Duration::from_secs(8), request)
                .await
                .context("YouTube page request timed out")?
        } else {
            request.await
        };
        let response = match response {
            Ok(http::Response::Ok(body)) => cache.store(&url, headers, &body)?,
            Ok(http::Response::NotModified) => {
                cached.context("original page returned not modified without a cached response")?
            }
            Err(err) => match cached {
                Some(cached) => {
                    log::debug!("using cached original page after fetch failed: {err:#}");
                    cached
                }
                None => return Err(err),
            },
        };
        if !http::is_html_content_type(response.content_type.as_deref()) {
            anyhow::bail!("original page is not HTML");
        }
        // Decoding and parsing the page is CPU work: do it once, off the runtime, and take every
        // page-derived fact from that single document.
        let wants_candidates = source.previews || source.images.archives();
        let audio = raw
            .extra
            .get("audio_url")
            .and_then(serde_yaml_ng::Value::as_str)
            .map(str::to_owned);
        let requested = url.clone();
        let (response, analysis) = tokio::task::spawn_blocking(move || {
            let analysis = analyze_page(
                response.html_text(),
                &response.final_url,
                &requested,
                audio.as_deref(),
                wants_candidates,
            );
            (response, analysis)
        })
        .await
        .context("analysing the original page")?;
        duration = analysis.duration;
        publisher = analysis.publisher;
        interactive = analysis.interactive;
        if wants_candidates {
            page_candidates = analysis.candidates;
        }
        if !analysis.activity_alternates.is_empty() {
            match crate::threads::expand_alternates(
                analysis.activity_alternates,
                source,
                client,
                &cache,
            )
            .await
            {
                Ok(Some(expanded)) => return Ok(Some(expanded)),
                Ok(None) => {}
                Err(err) => log::debug!(
                    "{}: ActivityPub thread expansion failed for {}: {err:#}",
                    source.slug,
                    response.final_url
                ),
            }
        }
        let page = analysis.page;
        let video = sources::youtube::is_video_url(&url);
        // Sentences of the feed entry the page repeats, taken before the page is consumed. An
        // entry with only a summary counts too: that summary becomes the body when no page
        // content is kept, which is exactly the outcome the check protects.
        let feed_entry = raw.content_html.as_deref().or(raw.summary.as_deref());
        let feed_on_page = match feed_entry {
            Some(feed) if !video => content::feed_content_on_page(feed, &page),
            _ => Vec::new(),
        };
        let extraction_key = response.extraction_key();
        // A page that is only a script shell keeps its article in its own module scripts.
        let modules = if !video && content::is_script_shell(&page) {
            content::module_scripts(&page, &response.final_url)
        } else {
            Vec::new()
        };
        let extracted = match cache.extracted(&extraction_key, &response.final_url)? {
            Some(extracted) => extracted,
            None => {
                let extracted = if video {
                    sources::youtube::extract(&page, &url, raw.content_html.as_deref(), client)
                        .await
                } else {
                    match content::extract_article_async(page, response.final_url.clone()).await {
                        Ok(extracted) => extracted,
                        Err(error) if !modules.is_empty() => {
                            article_from_modules(&modules, raw, source, client, &response.final_url)
                                .await
                                .with_context(|| format!("{error:#}"))?
                        }
                        Err(error) => return Err(error),
                    }
                };
                if !video || extracted.html != raw.content_html.as_deref().unwrap_or_default() {
                    cache.store_extracted(&extraction_key, &response.final_url, &extracted)?;
                }
                extracted
            }
        };
        if let Some(feed) = feed_entry
            && content::extraction_misses_feed_content(&extracted.html, feed, &feed_on_page)
        {
            log::debug!(
                "{}: the readable region of {} is not the entry the feed carries; keeping feed content",
                source.slug,
                response.final_url
            );
            return Ok(None);
        }
        let mut extracted = extracted;
        if content::is_subscription_wall(&extracted.html, &url) {
            match archive::recover(&url, &raw.title, client, &cache).await {
                Some(recovered) => {
                    extracted = recovered.article;
                    page_candidates = preview::html_candidate_groups(&extracted.html, &url);
                    archive_provenance = Some((recovered.snapshot, recovered.captured_at));
                }
                None => {
                    subscription_required = true;
                    page_candidates = preview::HtmlCandidateGroups::default();
                    return Ok(None);
                }
            }
        }
        extracted.html = crate::threads::expand_embedded_x(&extracted.html, source, client, &cache).await;
        Ok(Some(extracted))
    }
    .await;
    let result = match result {
        Err(error) if known_subscription => {
            failures.record(&url, http::status_code(&error));
            let cache = crate::cache::ArticleCache::new(cache_dir);
            if let Some(recovered) = archive::recover(&url, &raw.title, client, &cache).await {
                page_candidates = preview::html_candidate_groups(&recovered.article.html, &url);
                archive_provenance = Some((recovered.snapshot, recovered.captured_at));
                Ok(Some(recovered.article))
            } else {
                subscription_required = true;
                page_candidates = preview::HtmlCandidateGroups::default();
                Ok(None)
            }
        }
        other => other,
    };
    match result {
        // The page was fetched but its readable region is not the item: the feed entry (its
        // content, or its summary when that is all it has) is the article, and the page still
        // supplies what it knows about media and duration.
        Ok(None) => {
            let mut enriched = raw.clone();
            if subscription_required {
                enriched
                    .extra
                    .insert("subscription_required".into(), true.into());
                enriched.extra.insert(
                    "archive_lookup_url".into(),
                    content::archive_lookup_url(&url).into(),
                );
            }
            if let Some(seconds) = duration {
                enriched
                    .extra
                    .insert("duration_seconds".into(), seconds.into());
            }
            if let Some(profile) = publisher.clone() {
                enriched
                    .extra
                    .insert(crate::platform::PUBLISHER_KEY.into(), profile.into());
            }
            if interactive {
                enriched
                    .extra
                    .insert(crate::site::interactive::METADATA_KEY.into(), true.into());
            }
            if source.previews || source.images.archives() {
                enriched.preview_candidates =
                    preview::ordered_article_candidates(&preview_candidates, page_candidates, None);
            }
            (enriched, fallback)
        }
        Ok(Some(extracted)) => {
            let mut enriched = raw.clone();
            for key in [
                "subscription_required",
                "archive_lookup_url",
                "archive_url",
                "archive_captured_at",
            ] {
                enriched.extra.remove(key);
            }
            if let Some((snapshot, captured_at)) = archive_provenance {
                enriched.extra.insert("archive_url".into(), snapshot.into());
                if let Some(captured_at) = captured_at {
                    enriched
                        .extra
                        .insert("archive_captured_at".into(), captured_at.into());
                }
            }
            if let Some(link) = thread_link {
                enriched.link = link.to_string();
            }
            if let Some(seconds) = duration {
                enriched
                    .extra
                    .insert("duration_seconds".into(), seconds.into());
            }
            if let Some(profile) = publisher.clone() {
                enriched
                    .extra
                    .insert(crate::platform::PUBLISHER_KEY.into(), profile.into());
            }
            if interactive {
                enriched
                    .extra
                    .insert(crate::site::interactive::METADATA_KEY.into(), true.into());
            }
            enriched.labels =
                crate::model::normalize_labels(enriched.labels.iter().chain(&extracted.labels));
            enriched.content_html = Some(extracted.html);
            if source.previews || source.images.archives() {
                enriched.preview_candidates = preview::ordered_article_candidates(
                    &preview_candidates,
                    page_candidates,
                    extracted.image,
                );
            }
            (enriched, ContentKind::Extracted)
        }
        Err(err) => {
            let status = http::status_code(&err);
            failures.record(&url, status);
            log::debug!(
                "{}: original page unavailable for {}; keeping {}: {err:#}{}",
                source.slug,
                raw.link,
                if fallback == ContentKind::Feed {
                    "feed content"
                } else {
                    "item metadata and original link"
                },
                if status == Some(429) {
                    "; pausing original-page requests to this origin for this run"
                } else {
                    ""
                }
            );
            let mut enriched = raw.clone();
            if let Some(seconds) = duration {
                enriched
                    .extra
                    .insert("duration_seconds".into(), seconds.into());
            }
            if let Some(profile) = publisher.clone() {
                enriched
                    .extra
                    .insert(crate::platform::PUBLISHER_KEY.into(), profile.into());
            }
            if interactive {
                enriched
                    .extra
                    .insert(crate::site::interactive::METADATA_KEY.into(), true.into());
            }
            enriched.preview_candidates =
                preview::ordered_article_candidates(&preview_candidates, page_candidates, None);
            (enriched, fallback)
        }
    }
}

async fn hydrate_new_mirror_companions(
    raw: RawItem,
    known: bool,
    dry_run: bool,
    previews: bool,
    images: bool,
) -> RawItem {
    hydrate_new_mirror_companions_with(raw, known, dry_run, previews, images, |raw| {
        crate::sources::aggr::hydrate_companions(raw)
    })
    .await
}

async fn hydrate_new_mirror_companions_with<F, Fut>(
    mut raw: RawItem,
    known: bool,
    dry_run: bool,
    previews: bool,
    images: bool,
    hydrate: F,
) -> RawItem
where
    F: FnOnce(RawItem) -> Fut,
    Fut: std::future::Future<Output = RawItem>,
{
    if known || dry_run || (!previews && !images && documents::url(&raw.link, &raw.extra).is_none())
    {
        crate::sources::aggr::discard_companion_locator(&mut raw);
        raw
    } else {
        let mut raw = hydrate(raw).await;
        if !previews {
            raw.preview = None;
        }
        if !images {
            raw.images.clear();
        }
        raw
    }
}
