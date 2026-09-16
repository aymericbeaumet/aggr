//! Shared fetch stage for sync/build/dev: every source runs in parallel and writes only its own
//! directory. Source-local keys and shared URL reservations decide what is new; no git operations.

mod duration;
mod openreview;
mod podcast;
mod recording;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Utc};
use futures_util::{StreamExt as _, stream};
use tokio::sync::{Notify, OnceCell, Semaphore};
use tokio::task::JoinSet;

use super::Project;
use crate::cli::FetchArgs;
use crate::config::{ContentMode, Engine, Source, StoreConfig};
use crate::content;
use crate::git::Worktree;
use crate::http;
use crate::media;
use crate::model::{
    ContentKind, FrontMatter, RawItem, dedupe_keys, file_stem, item_dir, normalize_labels,
    normalize_link, unique_stem,
};
use crate::preview;
use crate::sources::{self, Fetch};
use crate::store::{NewItem, Outcome, SourceState, Store, retention};

pub struct Report {
    pub sources: Vec<SourceReport>,
    pub status_changed: bool,
    /// Items deleted by `[store]` retention.
    pub removed: usize,
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
        self.sources.iter().map(|s| s.added).sum()
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

#[derive(Clone)]
struct Options {
    archived_links: Arc<SharedLinks>,
    existing_paths: Arc<OnceCell<ExistingPaths>>,
    dry_run: bool,
    refresh: bool,
    reprocess: bool,
    html: bool,
    html_max_bytes: usize,
    article_concurrency: usize,
    preparation_limit: Arc<Semaphore>,
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
    let (known_links, existing_paths) =
        tokio::task::spawn_blocking(move || index_store.items().map(index_archive))
            .await
            .context("indexing archived article URLs and paths")??;
    let options = Options {
        archived_links: Arc::new(SharedLinks::new(known_links)),
        existing_paths: Arc::new(OnceCell::new_with(Some(existing_paths))),
        dry_run: args.dry_run,
        refresh: args.refresh,
        reprocess: args.reprocess,
        html: project.config.store.html,
        html_max_bytes: project.config.store.html_max_bytes,
        article_concurrency: project.config.fetch.article_concurrency,
        preparation_limit: Arc::new(Semaphore::new(2)),
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
    let mut message = format!("{err:#}");
    if let Some(private) = source.engine.url() {
        let replacement = source.public_url.as_deref().unwrap_or("[source URL]");
        message = message.replace(private.as_str(), replacement);
    }
    anyhow::anyhow!(message)
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

struct SourceTransaction {
    root: PathBuf,
    backup: Option<tempfile::TempDir>,
    tracked: HashSet<PathBuf>,
    snapshots: Vec<FileSnapshot>,
    finished: bool,
}

struct FileSnapshot {
    target: PathBuf,
    backup: Option<PathBuf>,
}

impl SourceTransaction {
    fn new(root: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(root)
            .with_context(|| format!("inspecting store root {}", root.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            bail!("store root is not a regular directory: {}", root.display());
        }
        Ok(Self {
            root: root
                .canonicalize()
                .with_context(|| format!("resolving store root {}", root.display()))?,
            backup: None,
            tracked: HashSet::new(),
            snapshots: Vec::new(),
            finished: false,
        })
    }

    fn track_relative(&mut self, relative: impl AsRef<Path>) -> Result<()> {
        let relative = relative.as_ref();
        let target = self.checked_target(relative)?;
        if !self.tracked.insert(target.clone()) {
            return Ok(());
        }
        let backup = match fs::symlink_metadata(&target) {
            Ok(metadata) => {
                debug_assert!(metadata.file_type().is_file());
                if self.backup.is_none() {
                    self.backup =
                        Some(tempfile::tempdir().context("creating source transaction backup")?);
                }
                let backup = self
                    .backup
                    .as_ref()
                    .context("source transaction backup was not initialized")?
                    .path()
                    .join(self.snapshots.len().to_string());
                fs::copy(&target, &backup)
                    .with_context(|| format!("backing up {}", target.display()))?;
                Some(backup)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", target.display()));
            }
        };
        self.snapshots.push(FileSnapshot { target, backup });
        Ok(())
    }

    fn checked_target(&self, relative: &Path) -> Result<PathBuf> {
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!("invalid source transaction path: {}", relative.display());
        }
        let mut parent = self.root.clone();
        for component in relative
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .components()
        {
            let Component::Normal(component) = component else {
                unreachable!("transaction path components were validated")
            };
            parent.push(component);
            match fs::symlink_metadata(&parent) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    bail!(
                        "source transaction parent is not a regular directory: {}",
                        parent.display()
                    )
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => {
                    return Err(error).with_context(|| format!("inspecting {}", parent.display()));
                }
            }
        }
        let target = self.root.join(relative);
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => bail!(
                "source transaction target is not a regular file: {}",
                target.display()
            ),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", target.display()));
            }
        }
        Ok(target)
    }

    fn track_item(&mut self, planned: &Planned, raw: &RawItem) -> Result<()> {
        let directory = Path::new(&planned.dir);
        self.track_relative(directory.join(format!("{}.md", planned.stem)))?;
        if planned.html.is_some() {
            self.track_relative(directory.join(format!("{}.html", planned.stem)))?;
        }
        if raw.preview.is_some()
            && let Some(preview) = &planned.front.preview
        {
            self.track_relative(directory.join(&preview.file))?;
        }
        for image in &raw.images {
            for companion in image.files(&planned.stem) {
                self.track_relative(directory.join(companion.metadata.file))?;
            }
        }
        Ok(())
    }

    fn track_seen(&mut self, slug: &str) -> Result<()> {
        self.track_relative(Path::new("sources").join(slug).join("seen.txt"))
    }

    fn track_state(&mut self, slug: &str) -> Result<()> {
        self.track_relative(Path::new("sources").join(slug).join("state.toml"))
    }

    fn commit(mut self) {
        self.finished = true;
    }

    fn rollback(&mut self) -> Result<()> {
        let mut first_error = None;
        for snapshot in self.snapshots.iter().rev() {
            let restored = (|| -> Result<()> {
                let relative = snapshot
                    .target
                    .strip_prefix(&self.root)
                    .context("transaction target escaped its store root")?;
                let target = self.checked_target(relative)?;
                match &snapshot.backup {
                    Some(backup) => {
                        match fs::symlink_metadata(&target) {
                            Ok(metadata) if metadata.file_type().is_file() => {
                                fs::remove_file(&target)
                                    .with_context(|| format!("removing {}", target.display()))?;
                            }
                            Ok(_) => bail!(
                                "refusing to replace non-file transaction target {}",
                                target.display()
                            ),
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            Err(error) => {
                                return Err(error)
                                    .with_context(|| format!("inspecting {}", target.display()));
                            }
                        }
                        if let Some(parent) = target.parent() {
                            fs::create_dir_all(parent)
                                .with_context(|| format!("recreating {}", parent.display()))?;
                        }
                        fs::copy(backup, &target)
                            .with_context(|| format!("restoring {}", target.display()))?;
                    }
                    None => match fs::remove_file(&target) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(error).with_context(|| {
                                format!("removing new file {}", target.display())
                            });
                        }
                    },
                }
                Ok(())
            })();
            if let Err(error) = restored {
                log::error!("source transaction rollback: {error:#}");
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        self.finished = true;
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for SourceTransaction {
    fn drop(&mut self) {
        if !self.finished
            && let Err(error) = self.rollback()
        {
            log::error!("source transaction rollback during drop: {error:#}");
        }
    }
}

#[derive(Clone, Copy)]
struct FetchOneContext<'a> {
    store: &'a Store,
    store_root: &'a Path,
    client: &'a http::Client,
    cache_dir: &'a Path,
    options: &'a Options,
    article_failures: &'a ArticleFailures,
    state_policy: StatePolicy,
}

#[cfg(test)]
fn archived_links(store: &Store) -> Result<HashSet<String>> {
    Ok(store
        .items()?
        .into_iter()
        .map(|item| normalize_link(&item.front.link))
        .filter(|link| !link.is_empty())
        .collect())
}

#[derive(Default)]
struct SharedLinks {
    state: Mutex<LinkState>,
    changed: Notify,
}

#[derive(Default)]
struct LinkState {
    committed: HashSet<String>,
    reserved: HashSet<String>,
}

impl SharedLinks {
    fn new(committed: HashSet<String>) -> Self {
        Self {
            state: Mutex::new(LinkState {
                committed,
                ..Default::default()
            }),
            changed: Notify::new(),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, LinkState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

#[derive(Default)]
struct LinkTransaction {
    shared: Option<Arc<SharedLinks>>,
    reserved: HashSet<String>,
}

impl LinkTransaction {
    async fn reserve(&mut self, shared: &Arc<SharedLinks>, wanted: HashSet<String>) {
        loop {
            let changed = shared.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = shared.state();
                if wanted.is_disjoint(&state.reserved) {
                    self.reserved = wanted.difference(&state.committed).cloned().collect();
                    state.reserved.extend(self.reserved.iter().cloned());
                    self.shared = Some(shared.clone());
                    return;
                }
            }
            changed.await;
        }
    }

    fn duplicate(&self, link: &str) -> bool {
        !link.is_empty() && !self.reserved.contains(link)
    }

    fn commit(self) {
        if let Some(shared) = &self.shared {
            shared
                .state()
                .committed
                .extend(self.reserved.iter().cloned());
        }
    }
}

impl Drop for LinkTransaction {
    fn drop(&mut self) {
        if let Some(shared) = &self.shared {
            {
                let mut state = shared.state();
                for link in &self.reserved {
                    state.reserved.remove(link);
                }
            }
            shared.changed.notify_waiters();
        }
    }
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
    let (mut report, mut visible_change) = match fetched {
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
            let metadata_changed =
                next_state.title != state.title || next_state.site_url != state.site_url;

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
            let mut enriched = stream::iter(candidates)
                .map(|(raw, keys, known, existing_path, stem)| async move {
                    let mut raw = hydrate_new_mirror_companions(
                        raw,
                        known,
                        options.dry_run,
                        source.previews,
                        source.images,
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
                        let has_stored_images = stored
                            .as_ref()
                            .is_some_and(|item| !item.front.images.is_empty());
                        if source.images
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
                        if source.images
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
                    store.write_item(NewItem {
                        dir: &planned.dir,
                        stem: &planned.stem,
                        front: &planned.front,
                        body: &planned.body,
                        html: planned.html.as_deref(),
                        preview: raw.preview.as_ref().map(|preview| preview.bytes.as_slice()),
                        images: &raw.images,
                    })?;
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
                added > 0,
            )
        }
    };
    let duration_repairs = repair_recordings(source, &context, transaction.as_deref_mut()).await?;
    let repaired = duration_repairs
        + repair_feed_captures(source, &context, transaction.as_deref_mut()).await?
        + repair_archived_images(source, store, options, transaction.as_deref_mut()).await?
        + reprocess_stored_bodies(source, store, options, transaction.as_deref_mut())?;
    report.added += repaired;
    report.unchanged &= repaired == 0;
    visible_change |= repaired > 0;
    if !options.dry_run && (visible_change || state_policy == StatePolicy::DevCache) {
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

fn merge_image_metadata(
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

/// Re-derive stored bodies from the HTML retained beside them. Content cleanup added after an item
/// was captured cannot reach it any other way: its source eventually stops listing it, and a build
/// never overrides a stored body. Explicit, like `--refresh`, because it discards hand edits, and
/// bounded to items whose retained HTML is complete so a body can never come back shorter.
fn reprocess_stored_bodies(
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
            existing.front.published,
            &source.slug,
        );
        if body == existing.body || body.trim().is_empty() {
            continue;
        }
        let mut planned = Planned {
            dir: String::new(),
            stem: String::new(),
            front: existing.front,
            body,
            html: None,
        };
        use_existing_path(&mut planned, path)?;
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

async fn repair_archived_images(
    source: &Source,
    store: &Store,
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
        let mut planned = Planned {
            dir: String::new(),
            stem: String::new(),
            front: existing.front,
            body: existing.body,
            html: None,
        };
        use_existing_path(&mut planned, &archive.path)?;
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
            store
                .write_item(NewItem {
                    dir: &planned.dir,
                    stem: &planned.stem,
                    front: &planned.front,
                    body: &planned.body,
                    html: None,
                    preview: raw.preview.as_ref().map(|preview| preview.bytes.as_slice()),
                    images: &raw.images,
                })
                .with_context(|| format!("repairing article companions for {}", archive.path))?;
        }
        repaired += 1;
    }
    Ok(repaired)
}

fn reconcile_podcast(
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
        let mut planned = Planned {
            dir: String::new(),
            stem: String::new(),
            front: existing.front,
            body: existing.body,
            html: None,
        };
        use_existing_path(&mut planned, path)?;
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

fn source_request_state(state: &SourceState, refresh: bool) -> SourceState {
    if refresh {
        SourceState::default()
    } else {
        state.clone()
    }
}

#[derive(Default)]
struct ExistingPaths {
    items: BTreeMap<String, BTreeMap<String, String>>,
    podcasts: podcast::Archive,
    images: BTreeMap<String, Vec<ArchivedImages>>,
    recordings: BTreeMap<String, Vec<(String, RawItem)>>,
    /// Items still carrying only feed content because the original page was unavailable.
    captures: BTreeMap<String, Vec<(String, RawItem)>>,
}

struct ArchivedImages {
    path: String,
    candidates: Vec<media::Candidate>,
    images: Vec<crate::model::ArticleImage>,
    preview: Option<crate::model::Preview>,
}

fn index_archive(
    items: impl IntoIterator<Item = crate::model::Item>,
) -> (HashSet<String>, ExistingPaths) {
    let mut links = HashSet::new();
    let mut paths = ExistingPaths::default();
    for item in items {
        use crate::site::item_type::ItemType;
        let audio = item
            .front
            .extra
            .get("audio_url")
            .and_then(serde_yaml_ng::Value::as_str);
        if item
            .front
            .extra
            .get("duration_seconds")
            .and_then(serde_yaml_ng::Value::as_u64)
            .is_none_or(|value| value == 0)
            && matches!(
                ItemType::from_urls(&item.front.link, audio),
                ItemType::Audio | ItemType::Video | ItemType::Podcast
            )
        {
            paths
                .recordings
                .entry(item.front.source.clone())
                .or_default()
                .push((
                    item.path.clone(),
                    RawItem {
                        title: item.front.title.clone(),
                        link: item.front.link.clone(),
                        extra: item.front.extra.clone(),
                        ..Default::default()
                    },
                ));
        }
        // Binary links are final as feed content: heavy extraction never requests them.
        if matches!(
            item.front.content,
            crate::model::ContentKind::Feed | crate::model::ContentKind::None
        ) && item.front.replicated_at.is_none()
            && url::Url::parse(&item.front.link).is_ok_and(|url| {
                matches!(url.scheme(), "http" | "https")
                    && !preview::is_pdf_url(&url)
                    && !url.path().rsplit_once('.').is_some_and(|(_, extension)| {
                        matches!(
                            extension.to_ascii_lowercase().as_str(),
                            "jpg" | "jpeg" | "png" | "webp" | "gif" | "avif"
                        )
                    })
            })
        {
            paths
                .captures
                .entry(item.front.source.clone())
                .or_default()
                .push((
                    item.path.clone(),
                    RawItem {
                        title: item.front.title.clone(),
                        link: item.front.link.clone(),
                        published: item.front.published,
                        updated: item.front.updated,
                        first_seen: Some(item.front.first_seen),
                        authors: item.front.authors.clone(),
                        labels: item.front.labels.clone(),
                        summary: item.front.summary.clone(),
                        extra: item.front.extra.clone(),
                        ..Default::default()
                    },
                ));
        }
        if let Ok(base) = url::Url::parse(&item.front.link) {
            let mut candidates = media::markdown_candidates(&item.body, &base);
            for image in &item.front.images {
                if let Ok(url) = url::Url::parse(&image.source)
                    && matches!(url.scheme(), "http" | "https")
                    && url.username().is_empty()
                    && url.password().is_none()
                    && !candidates.iter().any(|candidate| candidate.url == url)
                {
                    candidates.push(media::Candidate { url, alt: None });
                }
            }
            if !candidates.is_empty() {
                paths
                    .images
                    .entry(item.front.source.clone())
                    .or_default()
                    .push(ArchivedImages {
                        path: item.path.clone(),
                        candidates,
                        images: item.front.images.clone(),
                        preview: item.front.preview.clone(),
                    });
            }
        }
        paths.podcasts.insert(&item);
        let link = normalize_link(&item.front.link);
        if !link.is_empty() {
            links.insert(link);
        }
        let entries = paths.items.entry(item.front.source).or_default();
        let raw = RawItem {
            title: item.front.title,
            link: item.front.link,
            published: item.front.published,
            updated: item.front.updated,
            ..Default::default()
        };
        for key in dedupe_keys(&raw) {
            entries.entry(key).or_insert_with(|| item.path.clone());
        }
    }
    (links, paths)
}

#[cfg(test)]
fn index_existing_item_paths(
    items: impl IntoIterator<Item = crate::model::Item>,
    source_slug: &str,
) -> BTreeMap<String, String> {
    index_archive(items)
        .1
        .items
        .remove(source_slug)
        .unwrap_or_default()
}

fn use_existing_path(planned: &mut Planned, existing_path: &str) -> Result<()> {
    let path = Path::new(existing_path);
    let expected = Path::new("items").join(&planned.front.source);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || !path.starts_with(expected)
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

fn apply_validators(
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

fn keep_newest(items: &mut Vec<RawItem>, limit: usize) {
    items.sort_by_key(|item| std::cmp::Reverse(item.created_at()));
    items.truncate(limit);
}

fn apply_first_import_limit(items: &mut Vec<RawItem>, engine: &Engine, feed_limit: usize) {
    // Aggr sources already apply their own optional limit. With no limit, importing the full
    // retained tree is what makes one instance a useful replica of another.
    if matches!(engine, Engine::Feed { .. }) {
        keep_newest(items, feed_limit);
    }
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

async fn repair_recordings(
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
                .preparation_limit
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
async fn repair_feed_captures(
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
    let mut upgraded = 0;
    while let Some((path, mut raw, kind)) = pending.next().await {
        if kind != ContentKind::Extracted || raw.content_html.is_none() {
            log::debug!(
                "{}: original page still unavailable for {}; keeping feed content",
                source.slug,
                raw.link
            );
            retries.failed(&raw.link);
            continue;
        }
        let existing = context.store.read_item(&path)?;
        if existing.front.source != source.slug || existing.front.content == ContentKind::Extracted
        {
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
        planned.front.first_seen = existing.front.first_seen;
        planned.front.labels = existing.front.labels.clone();
        planned.front.authors = existing.front.authors.clone();
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
        if let Some(transaction) = transaction.as_deref_mut() {
            transaction.track_item(&planned, &raw)?;
        }
        context.store.write_item(NewItem {
            dir: &planned.dir,
            stem: &planned.stem,
            front: &planned.front,
            body: &planned.body,
            html: planned.html.as_deref(),
            preview: raw.preview.as_ref().map(|preview| preview.bytes.as_slice()),
            images: &raw.images,
        })?;
        retries.succeeded(&raw.link);
        log::info!(
            "{}: captured the original article for {} after an earlier failure",
            source.slug,
            raw.link
        );
        upgraded += 1;
    }
    Ok(upgraded)
}

fn recording_duration(page: &str, url: &url::Url, raw: &RawItem) -> Option<u64> {
    if sources::youtube::is_video_url(url) {
        return sources::youtube::duration_seconds(page, url);
    }
    let audio = raw
        .extra
        .get("audio_url")
        .and_then(serde_yaml_ng::Value::as_str);
    use crate::site::item_type::ItemType;
    if !matches!(
        ItemType::from_urls(url.as_str(), audio),
        ItemType::Video | ItemType::Audio | ItemType::Podcast
    ) {
        return None;
    }
    let media: Vec<_> = audio
        .and_then(|value| url::Url::parse(value).ok())
        .into_iter()
        .collect();
    crate::media_duration::from_html(page, url, &media)
}

fn reconcile_duration(
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
    let mut planned = Planned {
        dir: String::new(),
        stem: String::new(),
        front: item.front,
        body: item.body,
        html: None,
    };
    use_existing_path(&mut planned, path)?;
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
    if preview::is_pdf_url(&url)
        || url.path().rsplit_once('.').is_some_and(|(_, extension)| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "webp" | "gif" | "avif"
            )
        })
    {
        return (raw.clone(), fallback);
    }
    if failures.blocked(&url) {
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
    let mut preview_candidates = raw.preview_candidates.clone();
    let mut page_candidates = preview::HtmlCandidateGroups::default();
    let mut interactive = false;
    let mut duration = None;
    let mut thread_link = None;
    let result = async {
        let cache = crate::cache::ArticleCache::new(cache_dir);
        match crate::threads::expand_x(&url, source, client, &cache).await {
            Ok(Some(expanded)) => {
                thread_link = crate::threads::canonical_x_url(&url);
                preview_candidates.clear();
                page_candidates = preview::html_candidate_groups(&expanded.html, &url);
                return Ok(expanded);
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
        let page = response.html_text();
        duration = recording_duration(
            &page,
            if sources::youtube::is_video_url(&url) {
                &url
            } else {
                &response.final_url
            },
            raw,
        );
        interactive = crate::site::interactive::is_interactive_document(&page);
        if source.previews || source.images {
            page_candidates = preview::html_candidate_groups(&page, &response.final_url);
        }
        match crate::threads::expand(&page, &response.final_url, source, client, &cache).await {
            Ok(Some(expanded)) => return Ok(expanded),
            Ok(None) => {}
            Err(err) => log::debug!(
                "{}: ActivityPub thread expansion failed for {}: {err:#}",
                source.slug,
                response.final_url
            ),
        }
        let extraction_key = response.extraction_key();
        if let Some(extracted) = cache.extracted(&extraction_key, &response.final_url)? {
            return Ok(extracted);
        }
        let extracted = if sources::youtube::is_video_url(&url) {
            sources::youtube::extract(&page, &url, raw.content_html.as_deref(), client).await
        } else {
            content::extract_article_async(page, response.final_url.clone()).await?
        };
        if !sources::youtube::is_video_url(&url)
            || extracted.html != raw.content_html.as_deref().unwrap_or_default()
        {
            cache.store_extracted(&extraction_key, &response.final_url, &extracted)?;
        }
        Ok(extracted)
    }
    .await;
    match result {
        Ok(extracted) => {
            let mut enriched = raw.clone();
            if let Some(link) = thread_link {
                enriched.link = link.to_string();
            }
            if let Some(seconds) = duration {
                enriched
                    .extra
                    .insert("duration_seconds".into(), seconds.into());
            }
            if interactive {
                enriched
                    .extra
                    .insert(crate::site::interactive::METADATA_KEY.into(), true.into());
            }
            enriched.content_html = Some(extracted.html);
            if source.previews || source.images {
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
    if known || dry_run || (!previews && !images) {
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

/// An item's files, decided before anything is written.
struct Planned {
    dir: String,
    stem: String,
    front: FrontMatter,
    body: String,
    html: Option<String>,
}

async fn prepare_item(
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

fn plan(raw: &RawItem, source: &Source, options: &Options, content_kind: ContentKind) -> Planned {
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
    let body = content::strip_article_metadata(&body, published, &source.slug);
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use httpmock::prelude::*;
    use url::Url;

    pub(super) fn source() -> Source {
        Source {
            slug: "blog".into(),
            name: None,
            category: None,
            labels: vec![],
            identity: "blog".into(),
            public_url: Some("https://blog.example/feed".into()),
            persist_endpoint: true,
            headers: vec![],
            html: true,
            content: ContentMode::Heavy,
            previews: false,
            images: true,
            engine: crate::config::Engine::Feed {
                url: Url::parse("https://blog.example/feed").unwrap(),
            },
        }
    }

    #[tokio::test]
    async fn mirrored_articles_never_download_the_original_even_in_heavy_mode() {
        let server = MockServer::start();
        let original = server.mock(|when, then| {
            when.path("/article");
            then.status(500);
        });
        let mut source = source();
        source.engine = Engine::Aggr {
            url: Url::parse("https://github.com/example/archive.git").unwrap(),
            branch: "aggr".into(),
            sources: Vec::new(),
            limit: None,
        };
        let raw = RawItem {
            link: server.url("/article"),
            content_html: Some("<p>Already archived</p>".into()),
            ..Default::default()
        };
        let cache = tempfile::tempdir().unwrap();
        let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let (mirrored, kind) = heavy_content(
            &raw,
            &source,
            &client,
            cache.path(),
            &ArticleFailures::default(),
        )
        .await;
        assert_eq!(mirrored, raw);
        assert_eq!(kind, ContentKind::Feed);
        original.assert_calls(0);
    }

    #[tokio::test]
    async fn changed_mirror_hydrates_only_new_writable_companions() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let calls = AtomicUsize::new(0);
        let mut known = RawItem {
            title: "Known with corrupt companion".into(),
            link: "https://example.com/known".into(),
            ..Default::default()
        };
        crate::sources::aggr::attach_companion_locator(
            &mut known,
            Path::new("/instrumented-mirror"),
            "items/source/known-corrupt",
        )
        .unwrap();
        let known = hydrate_new_mirror_companions_with(known, true, false, true, true, |_| async {
            calls.fetch_add(1, Ordering::SeqCst);
            panic!("known mirror companion must not be read")
        })
        .await;
        assert!(known.images.is_empty());

        let mut new = RawItem {
            title: "New".into(),
            link: "https://example.com/new".into(),
            ..Default::default()
        };
        crate::sources::aggr::attach_companion_locator(
            &mut new,
            Path::new("/instrumented-mirror"),
            "items/source/new",
        )
        .unwrap();
        let new =
            hydrate_new_mirror_companions_with(new, false, false, true, true, |mut raw| async {
                calls.fetch_add(1, Ordering::SeqCst);
                crate::sources::aggr::discard_companion_locator(&mut raw);
                raw
            })
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(new.images.is_empty());

        let dry_run = RawItem {
            title: "Dry run".into(),
            link: "https://example.com/dry".into(),
            ..Default::default()
        };
        let _ = hydrate_new_mirror_companions_with(dry_run, false, true, true, true, |_| async {
            calls.fetch_add(1, Ordering::SeqCst);
            panic!("dry-run mirror companion must not be read")
        })
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn mirrored_companions_respect_source_media_options() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let calls = AtomicUsize::new(0);
        let mirrored = || {
            let mut raw = RawItem {
                title: "Mirrored".into(),
                link: "https://example.com/mirrored".into(),
                ..Default::default()
            };
            crate::sources::aggr::attach_companion_locator(
                &mut raw,
                Path::new("/instrumented-mirror"),
                "items/source/mirrored",
            )
            .unwrap();
            raw
        };
        let mut preview_bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(40, 20)
            .write_to(&mut preview_bytes, image::ImageFormat::Png)
            .unwrap();
        let preview = crate::preview::thumbnail(&preview_bytes.into_inner(), None).unwrap();
        let image = crate::media::Asset {
            source_url: "https://example.com/image.png".into(),
            source_hash: "source".into(),
            alt: None,
            master_bytes: vec![1],
            master_extension: "png",
            master_hash: "master".into(),
            width: 40,
            height: 20,
            dominant_color: "#000000".into(),
            placeholder: crate::media::placeholder::from_bytes(&preview.bytes).unwrap(),
            renditions: Vec::new(),
        };
        let disabled =
            hydrate_new_mirror_companions_with(mirrored(), false, false, false, false, |_| async {
                calls.fetch_add(1, Ordering::SeqCst);
                panic!("disabled mirror companions must not be read")
            })
            .await;
        assert!(disabled.preview.is_none());
        assert!(disabled.images.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let without_previews = hydrate_new_mirror_companions_with(
            mirrored(),
            false,
            false,
            false,
            true,
            |mut raw| async {
                calls.fetch_add(1, Ordering::SeqCst);
                raw.preview = Some(preview.clone());
                raw.images.push(image.clone());
                raw
            },
        )
        .await;
        assert!(without_previews.preview.is_none());
        assert_eq!(without_previews.images.len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let without_images = hydrate_new_mirror_companions_with(
            mirrored(),
            false,
            false,
            true,
            false,
            |mut raw| async {
                calls.fetch_add(1, Ordering::SeqCst);
                raw.preview = Some(preview);
                raw.images.push(image);
                raw
            },
        )
        .await;
        assert!(without_images.preview.is_some());
        assert!(without_images.images.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

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

    fn options() -> Options {
        Options {
            archived_links: Arc::default(),
            existing_paths: Arc::default(),
            dry_run: false,
            refresh: false,
            reprocess: false,
            html: true,
            html_max_bytes: 1000,
            article_concurrency: 4,
            preparation_limit: Arc::new(Semaphore::new(2)),
            max_items_per_source: 200,
            preview_fetcher: Arc::new(preview::Fetcher::new().unwrap()),
            media_fetcher: Arc::new(
                media::Fetcher::new(
                    &crate::config::FetchConfig::default(),
                    media::MediaLimits::default(),
                )
                .unwrap(),
            ),
            now: Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap(),
        }
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
        let store = Store::open(root.path());
        let first_seen = "2026-07-16T12:00:00Z".parse().unwrap();
        let front = FrontMatter {
            title: "Why teens deserve safe AI".into(),
            source: "blog".into(),
            link: server.url("/post"),
            first_seen,
            labels: vec!["safety".into()],
            content: ContentKind::Feed,
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
            let store = Store::open(root.path());
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
        let store = Store::open(root.path()).with_image_cache(cache.path());
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
        let store = Store::open(root.path());
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
        let store = Store::open(root.path());
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

    #[test]
    fn archive_index_scans_items_once_and_keeps_metadata_paths_per_source() {
        let visited = std::cell::Cell::new(0);
        let items = ["first", "second"].map(|source| crate::model::Item {
            path: format!("items/{source}/story"),
            front: FrontMatter {
                source: source.into(),
                title: "Same title".into(),
                link: "https://example.com/shared#fragment".into(),
                ..Default::default()
            },
            body: String::new(),
        });
        let (links, paths) = index_archive(
            items
                .clone()
                .into_iter()
                .inspect(|_| visited.set(visited.get() + 1)),
        );
        assert_eq!(visited.get(), 2);
        assert_eq!(links.len(), 1);
        assert_eq!(paths.items.len(), 2);
        for (source, entries) in &paths.items {
            assert!(!entries.is_empty());
            assert!(
                entries
                    .values()
                    .all(|path| path == &format!("items/{source}/story"))
            );
        }
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
    fn refresh_reuses_the_exact_collision_suffixed_item_path() {
        let published = Utc.with_ymd_and_hms(2026, 9, 1, 8, 0, 0).unwrap();
        let first = RawItem {
            title: "Same title".into(),
            link: "https://example.com/first".into(),
            published: Some(published),
            ..Default::default()
        };
        let second = RawItem {
            link: "https://example.com/second".into(),
            ..first.clone()
        };
        let base_stem = file_stem(published, &first.title);
        let second_keys = dedupe_keys(&second);
        let second_stem = unique_stem(&base_stem, &second_keys[0], |stem| stem == base_stem);
        let item = |path: String, raw: &RawItem| crate::model::Item {
            path,
            front: FrontMatter {
                title: raw.title.clone(),
                link: raw.link.clone(),
                source: "blog".into(),
                published: raw.published,
                first_seen: published,
                ..Default::default()
            },
            body: String::new(),
        };
        let indexed = index_existing_item_paths(
            [
                item(format!("items/blog/2026/09/{base_stem}"), &first),
                item(format!("items/blog/2026/09/{second_stem}"), &second),
            ],
            "blog",
        );
        let existing = second_keys.iter().find_map(|key| indexed.get(key)).unwrap();
        let mut planned = plan(&second, &source(), &options(), ContentKind::Feed);

        use_existing_path(&mut planned, existing).unwrap();

        assert_eq!(planned.stem, second_stem);
        assert_ne!(planned.stem, base_stem);
    }

    #[tokio::test]
    async fn slow_first_article_does_not_block_later_downloads_or_change_filenames() {
        use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

        crate::http::install_crypto_provider();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let release_first = Arc::new(Semaphore::new(0));
        let (requests, mut received) = tokio::sync::mpsc::unbounded_channel();
        let gate = release_first.clone();
        let articles = tokio::spawn(async move {
            let mut responses = JoinSet::new();
            for _ in 0..3 {
                let (socket, _) = listener.accept().await.unwrap();
                let requests = requests.clone();
                let gate = gate.clone();
                responses.spawn(async move {
                    let mut socket = BufReader::new(socket);
                    let mut line = String::new();
                    socket.read_line(&mut line).await.unwrap();
                    let path = line.split_whitespace().nth(1).unwrap().to_string();
                    loop {
                        line.clear();
                        socket.read_line(&mut line).await.unwrap();
                        if line == "\r\n" { break; }
                    }
                    requests.send(path.clone()).unwrap();
                    if path == "/first" {
                        let _permit = gate.acquire().await.unwrap();
                    }
                    let body = format!("<article><h1>Same</h1><p>This is the complete article for {path}, with enough original readable text to preserve its contents.</p><p>The next paragraph supplies further relevant detail for this article.</p></article>");
                    socket.get_mut().write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                });
            }
            while responses.join_next().await.is_some() {}
        });
        let server = MockServer::start_async().await;
        server.mock_async(|when, then| {
            when.path("/feed");
            then.status(200).header("content-type", "application/feed+json").json_body(serde_json::json!({
                "version": "https://jsonfeed.org/version/1.1", "title": "Slow article test",
                "items": ([("third", 3), ("second", 2), ("first", 1)].into_iter().map(|(path, hour)| serde_json::json!({
                    "id": path, "title": "Same", "url": format!("{base}/{path}"),
                    "date_published": format!("2026-09-01T0{hour}:00:00Z"), "content_text": "Excerpt"
                })).collect::<Vec<_>>())
            }));
        }).await;
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let store = Store::open(root.path());
        let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let configured = Source {
            images: false,
            engine: Engine::Feed {
                url: Url::parse(&server.url("/feed")).unwrap(),
            },
            ..source()
        };
        let test_options = Options {
            article_concurrency: 2,
            ..options()
        };
        let failures = ArticleFailures::default();
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(
                fetch_one(
                    &configured,
                    FetchOneContext {
                        store: &store,
                        store_root: root.path(),
                        client: &client,
                        cache_dir: cache.path(),
                        options: &test_options,
                        article_failures: &failures,
                        state_policy: StatePolicy::PersistentBranch,
                    }
                ),
                async {
                    while let Some(path) = received.recv().await {
                        if path == "/third" {
                            release_first.add_permits(1);
                            return;
                        }
                    }
                    panic!("third article was never requested");
                }
            )
        })
        .await;
        articles.abort();
        let (report, ()) =
            result.expect("third article must start while the first is still blocked");
        assert_eq!(report.unwrap().added, 3);
        let stored = store.items().unwrap();
        let first = stored
            .iter()
            .find(|item| item.front.link.ends_with("/first"))
            .unwrap();
        assert!(first.path.ends_with("/2026-09-01-same"), "{}", first.path);
        assert_eq!(
            stored
                .iter()
                .map(|item| &item.path)
                .collect::<HashSet<_>>()
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn link_reservations_wait_only_for_overlap_and_retry_after_rollback() {
        use futures_util::FutureExt as _;

        let shared = Arc::new(SharedLinks::default());
        let mut first = LinkTransaction::default();
        first.reserve(&shared, HashSet::from(["a".into()])).await;
        let mut disjoint = LinkTransaction::default();
        disjoint
            .reserve(&shared, HashSet::from(["b".into()]))
            .now_or_never()
            .expect("unrelated articles must remain concurrent");
        disjoint.commit();
        let waiting_shared = shared.clone();
        let waiting = tokio::spawn(async move {
            let mut next = LinkTransaction::default();
            next.reserve(&waiting_shared, HashSet::from(["a".into(), "b".into()]))
                .await;
            assert!(!next.duplicate("a"));
            assert!(next.duplicate("b"));
            next.commit();
        });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        drop(first);
        tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            shared.state().committed,
            HashSet::from(["a".into(), "b".into()])
        );
        assert!(shared.state().reserved.is_empty());
    }

    #[tokio::test]
    async fn concurrent_sources_store_a_shared_article_once_and_duplicates_leave_no_trace() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        for (path, id, link) in [
            (
                "/publisher",
                "publisher-id",
                "https://www.example.com/article/",
            ),
            (
                "/aggregator",
                "hn-id",
                "http://example.com/article?utm_source=hn#comments",
            ),
        ] {
            server
                .mock_async(|when, then| {
                    when.path(path);
                    then.status(200)
                        .header("content-type", "application/feed+json")
                        .json_body(serde_json::json!({
                            "version": "https://jsonfeed.org/version/1.1",
                            "title": id,
                            "items": [{"id": id, "title": id, "url": link, "content_text": "Body"}]
                        }));
                })
                .await;
        }
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let store = Store::open(root.path());
        let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let configured = |slug: &str| Source {
            slug: slug.into(),
            content: ContentMode::Light,
            images: false,
            engine: Engine::Feed {
                url: Url::parse(&server.url(format!("/{slug}"))).unwrap(),
            },
            ..source()
        };
        let publisher = configured("publisher");
        let aggregator = configured("aggregator");
        let test_options = options();
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
        let dry_options = Options {
            dry_run: true,
            ..options()
        };
        let dry_context = FetchOneContext {
            options: &dry_options,
            ..context
        };
        let (first, second) = tokio::join!(
            fetch_one(&publisher, dry_context),
            fetch_one(&aggregator, dry_context)
        );
        assert_eq!(first.unwrap().added + second.unwrap().added, 1);
        assert!(store.items().unwrap().is_empty());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
        let (first, second) = tokio::join!(
            fetch_one(&publisher, context),
            fetch_one(&aggregator, context)
        );
        assert_eq!(first.unwrap().added + second.unwrap().added, 1);
        let archived = store.items().unwrap();
        assert_eq!(archived.len(), 1);
        let skipped = if archived[0].front.source == publisher.slug {
            &aggregator
        } else {
            &publisher
        };
        assert!(!root.path().join("sources").join(&skipped.slug).exists());
        let next_options = Options {
            archived_links: Arc::new(SharedLinks::new(archived_links(&store).unwrap())),
            ..options()
        };
        let next_context = FetchOneContext {
            options: &next_options,
            ..context
        };
        assert_eq!(fetch_one(skipped, next_context).await.unwrap().added, 0);
        assert!(!root.path().join("sources").join(&skipped.slug).exists());
        let owner = if skipped.slug == publisher.slug {
            &aggregator
        } else {
            &publisher
        };
        let refresh_options = Options {
            refresh: true,
            ..next_options.clone()
        };
        assert_eq!(
            fetch_one(
                owner,
                FetchOneContext {
                    options: &refresh_options,
                    ..context
                }
            )
            .await
            .unwrap()
            .added,
            1
        );
        assert_eq!(store.items().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn failed_source_rolls_back_earlier_items_and_seen_state() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        let feed = server
            .mock_async(|when, then| {
                when.method(GET).path("/feed.json");
                then.status(200)
                    .header("content-type", "application/feed+json")
                    .json_body(serde_json::json!({
                        "version": "https://jsonfeed.org/version/1.1",
                        "title": "Transaction test",
                        "items": [
                            {
                                "id": "second",
                                "title": "Second",
                                "url": server.url("/second"),
                                "date_published": "2026-09-02T08:00:00Z",
                                "content_html": "<p>Second body.</p>"
                            },
                            {
                                "id": "first",
                                "title": "First",
                                "url": server.url("/first"),
                                "date_published": "2026-08-31T08:00:00Z",
                                "content_html": "<p>First body.</p>"
                            }
                        ]
                    }));
            })
            .await;
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path());
        let invalid = root.path().join("items/blog/2026/09");
        fs::create_dir_all(invalid.parent().unwrap()).unwrap();
        fs::write(&invalid, "not a directory").unwrap();
        let cache = tempfile::tempdir().unwrap();
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let mut configured = source();
        configured.content = ContentMode::Light;
        configured.html = false;
        configured.previews = false;
        configured.images = false;
        configured.engine = Engine::Feed {
            url: Url::parse(&server.url("/feed.json")).unwrap(),
        };

        let test_options = Options {
            article_concurrency: 1,
            ..options()
        };
        let failures = ArticleFailures::default();
        let error = match fetch_one(
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
        {
            Ok(_) => panic!("the invalid second target should fail the source"),
            Err(error) => error,
        };

        assert!(
            format!("{error:#}").contains("source transaction parent is not a regular directory"),
            "{error:#}"
        );
        assert!(
            !root
                .path()
                .join("items/blog/2026/08/2026-08-31-first.md")
                .exists()
        );
        assert!(!root.path().join("sources/blog/seen.txt").exists());
        assert!(invalid.is_file());
        assert!(test_options.archived_links.state().committed.is_empty());
        assert!(test_options.archived_links.state().reserved.is_empty());
        configured.slug = "working".into();
        let retry = fetch_one(
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
        assert_eq!(retry.added, 2);
        assert_eq!(store.items().unwrap().len(), 2);
        feed.assert_calls_async(2).await;
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
        let store = Store::open(root.path());
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
        let store = Store::open(root.path());
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

    #[tokio::test]
    async fn light_youtube_videos_keep_feed_content_without_waiting_for_a_page_request() {
        use futures_util::FutureExt as _;

        crate::http::install_crypto_provider();
        let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let cache = tempfile::tempdir().unwrap();
        for link in [
            "https://www.youtube.com/watch?v=video",
            "https://youtu.be/video",
        ] {
            let raw = RawItem {
                title: "Video".into(),
                link: link.into(),
                content_html: Some("<p>Video description</p>".into()),
                ..Default::default()
            };
            let mut light = source();
            light.content = ContentMode::Light;
            let (kept, kind) = heavy_content(
                &raw,
                &light,
                &client,
                cache.path(),
                &ArticleFailures::default(),
            )
            .now_or_never()
            .expect("YouTube metadata must not await an article request");
            assert_eq!(kind, ContentKind::Feed);
            assert_eq!(kept.content_html, raw.content_html);
        }
        assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn binary_articles_keep_feed_content_without_a_duplicate_html_download() {
        use futures_util::FutureExt as _;
        crate::http::install_crypto_provider();
        let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let cache = tempfile::tempdir().unwrap();
        let mut heavy = source();
        heavy.content = ContentMode::Heavy;
        for link in [
            "https://example.com/paper.PDF?download=1",
            "https://example.com/paper%2Epdf",
            "https://example.com/download?filename=Paper%20One.PDF",
            "https://example.com/photo.png",
            "https://example.com/photo.JPEG#full",
        ] {
            let raw = RawItem {
                title: "Retained article".into(),
                link: link.into(),
                content_html: Some("<p>Existing feed description</p>".into()),
                ..Default::default()
            };
            let (kept, kind) = heavy_content(
                &raw,
                &heavy,
                &client,
                cache.path(),
                &ArticleFailures::default(),
            )
            .now_or_never()
            .expect("binary article previews must not trigger an earlier HTML download");
            assert_eq!(kind, ContentKind::Feed);
            assert_eq!(kept.content_html, raw.content_html);
            assert_eq!(kept.link, raw.link);
        }
        assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn qwen_full_api_articles_do_not_request_javascript_shells() {
        use futures_util::FutureExt as _;
        crate::http::install_crypto_provider();
        let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let cache = tempfile::tempdir().unwrap();
        let mut source = source();
        source.content = ContentMode::Heavy;
        source.engine = Engine::Feed {
            url: Url::parse("https://qwen.ai/blog").unwrap(),
        };
        let raw = RawItem {
            link: "https://qwen.ai/blog?id=release".into(),
            content_html: Some("<p>The complete API article.</p>".into()),
            ..Default::default()
        };
        let (kept, kind) = heavy_content(
            &raw,
            &source,
            &client,
            cache.path(),
            &ArticleFailures::default(),
        )
        .now_or_never()
        .expect("full Qwen API content must not await a page request");
        assert_eq!(kind, ContentKind::Feed);
        assert_eq!(kept.content_html, raw.content_html);
        assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn heavy_downloads_the_article_while_light_keeps_feed_content() {
        let server = MockServer::start_async().await;
        let article = server
            .mock_async(|when, then| {
                when.method(GET).path("/post");
                then.status(200).header("etag", "\"article-v1\"").body(
                    "<html><title>Post</title><article><h1>Post</h1><p>The complete original article has substantially more useful text than its feed excerpt.</p><p>This second paragraph makes it readable.</p></article></html>",
                );
            })
            .await;
        let raw = RawItem {
            title: "Post".into(),
            link: server.url("/post"),
            content_html: Some("<p>short feed excerpt</p>".into()),
            ..Default::default()
        };
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let cache = tempfile::tempdir().unwrap();

        let failures = ArticleFailures::default();
        let (heavy, kind) = heavy_content(&raw, &source(), &client, cache.path(), &failures).await;
        assert_eq!(kind, ContentKind::Extracted);
        assert!(
            heavy
                .content_html
                .unwrap()
                .contains("complete original article")
        );
        article.assert_calls_async(1).await;
        article.delete_async().await;
        let not_modified = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/post")
                    .header("if-none-match", "\"article-v1\"");
                then.status(304);
            })
            .await;
        let (cached, kind) = heavy_content(&raw, &source(), &client, cache.path(), &failures).await;
        assert_eq!(kind, ContentKind::Extracted);
        assert!(
            cached
                .content_html
                .unwrap()
                .contains("complete original article")
        );
        not_modified.assert_calls_async(1).await;

        let light = Source {
            content: ContentMode::Light,
            ..source()
        };
        let (unchanged, kind) = heavy_content(&raw, &light, &client, cache.path(), &failures).await;
        assert_eq!(kind, ContentKind::Feed);
        assert_eq!(unchanged.content_html, raw.content_html);
        not_modified.assert_calls_async(1).await;
    }

    #[tokio::test]
    async fn heavy_prefers_a_discovered_public_activitypub_self_thread() {
        let server = MockServer::start_async().await;
        let activity_url = server.url("/users/alice/statuses/100");
        let actor_url = server.url("/users/alice");
        let child_url = server.url("/users/alice/statuses/101");
        let page = server
            .mock_async(|when, then| {
                when.method(GET).path("/@alice/100");
                then.status(200).header("content-type", "text/html").body(format!(
                    r#"<html><head><link rel="alternate" type="application/activity+json" href="{activity_url}"></head><body><article><p>This ordinary fallback article contains enough readable text for extraction if thread expansion does not work.</p><p>It should not replace the discovered thread.</p></article></body></html>"#
                ));
            })
            .await;
        let activity = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/users/alice/statuses/100")
                    .header_matches("accept", ".*application/activity\\+json.*");
                then.status(200)
                    .header("content-type", "application/activity+json")
                    .json_body(serde_json::json!({
                        "id": activity_url,
                        "type": "Note",
                        "attributedTo": actor_url,
                        "to": ["https://www.w3.org/ns/activitystreams#Public"],
                        "content": "<p>Thread opening</p>",
                        "replies": {"items": [{
                            "id": child_url,
                            "type": "Note",
                            "attributedTo": actor_url,
                            "inReplyTo": activity_url,
                            "to": ["https://www.w3.org/ns/activitystreams#Public"],
                            "content": "<p>Thread continuation</p>",
                            "replies": {"items": []}
                        }]}
                    }));
            })
            .await;
        let raw = RawItem {
            title: "Thread".into(),
            link: server.url("/@alice/100"),
            content_html: Some("<p>short feed excerpt</p>".into()),
            ..Default::default()
        };
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let cache = tempfile::tempdir().unwrap();

        let (expanded, kind) = heavy_content(
            &raw,
            &source(),
            &client,
            cache.path(),
            &ArticleFailures::default(),
        )
        .await;

        assert_eq!(kind, ContentKind::Extracted);
        let html = expanded.content_html.unwrap();
        assert!(html.contains("Thread opening"), "{html}");
        assert!(html.contains("Thread continuation"), "{html}");
        assert!(!html.contains("ordinary fallback"), "{html}");
        page.assert_calls_async(1).await;
        activity.assert_calls_async(1).await;
    }

    #[tokio::test]
    async fn activitypub_failure_nonfatally_uses_normal_article_extraction() {
        let server = MockServer::start_async().await;
        let activity_url = server.url("/activity/100");
        let page = server
            .mock_async(|when, then| {
                when.method(GET).path("/post");
                then.status(200).header("content-type", "text/html").body(format!(
                    r#"<html><head><link rel="alternate" type="application/activity+json" href="{activity_url}"></head><body><article><h1>Fallback</h1><p>The complete ordinary article remains available when its advertised social representation cannot be loaded.</p><p>This second paragraph keeps the document readable.</p></article></body></html>"#
                ));
            })
            .await;
        let failed_activity = server
            .mock_async(|when, then| {
                when.method(GET).path("/activity/100");
                then.status(500);
            })
            .await;
        let raw = RawItem {
            title: "Fallback".into(),
            link: server.url("/post"),
            content_html: Some("<p>short feed excerpt</p>".into()),
            ..Default::default()
        };
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let cache = tempfile::tempdir().unwrap();

        let (fallback, kind) = heavy_content(
            &raw,
            &source(),
            &client,
            cache.path(),
            &ArticleFailures::default(),
        )
        .await;

        assert_eq!(kind, ContentKind::Extracted);
        assert!(
            fallback
                .content_html
                .unwrap()
                .contains("complete ordinary article")
        );
        page.assert_calls_async(1).await;
        failed_activity.assert_calls_async(1).await;
    }

    #[tokio::test]
    async fn heavy_stops_retrying_an_article_that_denies_requests() {
        let server = MockServer::start_async().await;
        let denied = server
            .mock_async(|when, then| {
                when.method(GET).path("/post");
                then.status(403);
            })
            .await;
        let raw = RawItem {
            title: "Post".into(),
            link: server.url("/post"),
            content_html: Some("<p>feed copy</p>".into()),
            ..Default::default()
        };
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let cache = tempfile::tempdir().unwrap();
        let failures = ArticleFailures::default();

        for _ in 0..3 {
            let (item, kind) =
                heavy_content(&raw, &source(), &client, cache.path(), &failures).await;
            assert_eq!(kind, ContentKind::Feed);
            assert_eq!(item.content_html, raw.content_html);
        }
        denied.assert_calls_async(1).await;
    }

    #[tokio::test]
    async fn article_denials_preserve_feed_items_and_allow_other_pages_on_the_origin() {
        for status in [401, 403] {
            let server = MockServer::start_async().await;
            let denied = server
                .mock_async(|when, then| {
                    when.method(GET).path("/private");
                    then.status(status);
                })
                .await;
            let allowed = server.mock_async(|when, then| {
                when.method(GET).path("/public");
                then.status(200).header("content-type", "text/html").body(
                    "<article><h1>Public article</h1><p>This complete public article remains accessible when another page on the same site requires authorization.</p><p>The feed must continue enriching its other items independently.</p></article>",
                );
            }).await;
            let raw = |path: &str| RawItem {
                title: "Article".into(),
                link: server.url(path),
                content_html: Some("<p>feed copy</p>".into()),
                ..Default::default()
            };
            let client = http::Client::new(&crate::config::FetchConfig {
                retries: 0,
                ..Default::default()
            })
            .unwrap();
            let cache = tempfile::tempdir().unwrap();
            let failures = ArticleFailures::default();
            let private = raw("/private");
            let (item, kind) =
                heavy_content(&private, &source(), &client, cache.path(), &failures).await;
            assert_eq!(kind, ContentKind::Feed);
            assert_eq!(item.link, private.link);
            assert_eq!(item.content_html, private.content_html);
            let (item, kind) =
                heavy_content(&raw("/public"), &source(), &client, cache.path(), &failures).await;
            assert_eq!(kind, ContentKind::Extracted);
            assert!(
                item.content_html
                    .unwrap()
                    .contains("complete public article")
            );
            denied.assert_calls_async(1).await;
            allowed.assert_calls_async(1).await;
        }
    }

    #[tokio::test]
    async fn article_rate_limits_preserve_items_and_pause_only_the_affected_origin() {
        let server = MockServer::start_async().await;
        let limited = server
            .mock_async(|when, then| {
                when.method(GET);
                then.status(429);
            })
            .await;
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let cache = tempfile::tempdir().unwrap();
        let failures = ArticleFailures::default();
        for path in ["/first", "/second"] {
            let raw = RawItem {
                title: "Article".into(),
                link: server.url(path),
                content_html: Some("<p>feed copy</p>".into()),
                ..Default::default()
            };
            let (item, kind) =
                heavy_content(&raw, &source(), &client, cache.path(), &failures).await;
            assert_eq!(kind, ContentKind::Feed);
            assert_eq!(item.content_html, raw.content_html);
            assert_eq!(item.link, raw.link);
        }
        limited.assert_calls_async(1).await;
        assert!(!failures.blocked(&Url::parse("https://other.example/post").unwrap()));
    }

    #[tokio::test]
    async fn article_denials_do_not_cross_source_credentials_on_one_origin() {
        let server = MockServer::start_async().await;
        let denied = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/private")
                    .header("authorization", "Bearer rejected");
                then.status(403);
            })
            .await;
        let allowed = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/public")
                    .header("authorization", "Bearer accepted");
                then.status(200).header("content-type", "text/html").body(
                    "<article><h1>Allowed</h1><p>This independently authorized source keeps its complete readable article.</p><p>It must not inherit another source's denial.</p></article>",
                );
            })
            .await;
        let make_source = |slug: &str, token: &str| {
            let mut source = source();
            source.slug = slug.into();
            source.headers = vec![("Authorization".into(), format!("Bearer {token}"))];
            source.engine = Engine::Feed {
                url: Url::parse(&server.url("/feed")).unwrap(),
            };
            source
        };
        let rejected = make_source("rejected", "rejected");
        let accepted = make_source("accepted", "accepted");
        let raw = |path: &str| RawItem {
            title: "Article".into(),
            link: server.url(path),
            content_html: Some("<p>feed copy</p>".into()),
            ..Default::default()
        };
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let cache = tempfile::tempdir().unwrap();
        let rejected_failures = ArticleFailures::default();
        let accepted_failures = ArticleFailures::default();

        let (_, rejected_kind) = heavy_content(
            &raw("/private"),
            &rejected,
            &client,
            cache.path(),
            &rejected_failures,
        )
        .await;
        let (article, accepted_kind) = heavy_content(
            &raw("/public"),
            &accepted,
            &client,
            cache.path(),
            &accepted_failures,
        )
        .await;
        assert_eq!(rejected_kind, ContentKind::Feed);
        assert_eq!(accepted_kind, ContentKind::Extracted);
        assert!(
            article
                .content_html
                .unwrap()
                .contains("independently authorized")
        );
        denied.assert_calls_async(1).await;
        allowed.assert_calls_async(1).await;
    }
}
