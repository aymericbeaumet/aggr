//! Shared fetch stage for sync/build/dev: every source runs in parallel and writes only its own
//! directory. The store's dedupe keys decide what is new; nothing in this module touches git.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Utc};
use futures_util::{StreamExt as _, stream};
use tokio::sync::Semaphore;
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
    unique_stem,
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
    dry_run: bool,
    refresh: bool,
    html: bool,
    html_max_bytes: usize,
    article_concurrency: usize,
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
    let store = Arc::new(Store::open(worktree.dir()));
    let client = Arc::new(http::Client::new(&project.config.fetch)?);
    let options = Options {
        dry_run: args.dry_run,
        refresh: args.refresh,
        html: project.config.store.html,
        html_max_bytes: project.config.store.html_max_bytes,
        article_concurrency: project.config.fetch.article_concurrency,
        max_items_per_source: project.config.fetch.max_items_per_source,
        preview_fetcher: Arc::new(preview::Fetcher::new()?),
        media_fetcher: Arc::new(media::Fetcher::new(
            &project.config.fetch,
            media::MediaLimits::default(),
        )?),
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
            (source.slug, result)
        });
    }

    let mut reports = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        let (slug, result) = joined.context("a fetch task panicked")?;
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

    for report in &reports {
        match &report.outcome {
            Outcome::Error(message) => println!("{}: error: {message}", report.slug),
            Outcome::Ok if report.unchanged => println!("{}: unchanged", report.slug),
            Outcome::Ok => println!("{}: +{}", report.slug, report.added),
        }
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

async fn fetch_one(source: &Source, context: FetchOneContext<'_>) -> Result<SourceReport> {
    let mut transaction = (!context.options.dry_run)
        .then(|| SourceTransaction::new(context.store_root))
        .transpose()?;
    let result = fetch_one_inner(source, context, transaction.as_mut()).await;
    match result {
        Ok(report) => {
            if let Some(transaction) = transaction {
                transaction.commit();
            }
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
) -> Result<SourceReport> {
    let FetchOneContext {
        store,
        client,
        cache_dir,
        options,
        article_failures,
        state_policy,
        ..
    } = context;
    let slug = &source.slug;
    let state = store.source_state(slug)?;
    let request_state = source_request_state(&state, options.refresh);
    let ctx = sources::Context {
        client,
        state: &request_state,
        cache_dir,
    };
    let fetched = sources::fetch(source, &ctx).await?;

    let mut next_state = state.clone();
    next_state.identity = source.identity.clone();
    let (report, visible_change) = match fetched {
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
            let existing_paths = if options.refresh {
                existing_item_paths(store, slug)?
            } else {
                BTreeMap::new()
            };
            let mut new_keys: Vec<String> = Vec::new();
            let mut prospective = seen.clone();
            let mut taken: HashSet<(String, String)> = HashSet::new();
            let mut added = 0;
            apply_first_import_limit(&mut items, &source.engine, options.max_items_per_source);
            // Feeds list newest first; preserve oldest-first writes while original pages download
            // concurrently. This keeps deterministic suffixes without making heavy mode serial.
            let mut candidates = Vec::new();
            for raw in items.into_iter().rev() {
                let keys = dedupe_keys(&raw);
                let known = keys.iter().any(|key| prospective.contains(key));
                if known && !options.refresh {
                    continue;
                }
                if !known {
                    prospective.extend(keys.iter().cloned());
                }
                let existing_path = known
                    .then(|| keys.iter().find_map(|key| existing_paths.get(key)).cloned())
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
            let mut enriched = stream::iter(candidates)
                .map(|(raw, keys, known, existing_path)| async move {
                    let raw = hydrate_new_mirror_companions(
                        raw,
                        known,
                        options.dry_run,
                        source.previews,
                        source.images,
                    )
                    .await;
                    let (mut raw, kind) =
                        heavy_content(&raw, source, client, cache_dir, article_failures).await;
                    if known {
                        raw.preview = None;
                        raw.images.clear();
                    } else if !matches!(source.engine, Engine::Aggr { .. }) {
                        if source.images
                            && raw.images.is_empty()
                            && let Some(html) = raw.content_html.as_deref()
                            && let Ok(base) = url::Url::parse(&raw.link)
                        {
                            let candidates = media::body_candidates(html, &base);
                            raw.images = options.media_fetcher.fetch(&candidates, source).await;
                        }
                        if source.previews
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
                        }
                    }
                    (raw, kind, keys, known, existing_path)
                })
                .buffered(options.article_concurrency);
            while let Some((raw, content_kind, keys, known, existing_path)) = enriched.next().await
            {
                let mut planned = plan(&raw, source, options, content_kind);
                if let Some(existing_path) = existing_path.as_deref() {
                    use_existing_path(&mut planned, existing_path)?;
                } else if !options.refresh {
                    let dir = planned.dir.clone();
                    let identity = keys.first().map(String::as_str).unwrap_or(&raw.link);
                    planned.stem = unique_stem(&planned.stem, identity, |stem| {
                        store.stem_exists(&dir, stem)
                            || taken.contains(&(dir.clone(), stem.to_string()))
                    });
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
                    planned.front.images = raw
                        .images
                        .iter()
                        .map(|image| image.metadata(&planned.stem))
                        .collect();
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
                taken.insert((planned.dir, planned.stem));
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
    if !options.dry_run && (visible_change || state_policy == StatePolicy::DevCache) {
        if let Some(transaction) = transaction {
            transaction.track_state(slug)?;
        }
        store.write_source_state(slug, &next_state)?;
    }
    Ok(report)
}

fn source_request_state(state: &SourceState, refresh: bool) -> SourceState {
    if refresh {
        SourceState::default()
    } else {
        state.clone()
    }
}

fn existing_item_paths(store: &Store, source_slug: &str) -> Result<BTreeMap<String, String>> {
    Ok(index_existing_item_paths(store.items()?, source_slug))
}

fn index_existing_item_paths(
    items: impl IntoIterator<Item = crate::model::Item>,
    source_slug: &str,
) -> BTreeMap<String, String> {
    let mut paths = BTreeMap::new();
    for item in items
        .into_iter()
        .filter(|item| item.front.source == source_slug)
    {
        let raw = RawItem {
            title: item.front.title,
            link: item.front.link,
            published: item.front.published,
            updated: item.front.updated,
            ..Default::default()
        };
        for key in dedupe_keys(&raw) {
            paths.entry(key).or_insert_with(|| item.path.clone());
        }
    }
    paths
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

#[derive(Default)]
struct ArticleFailures {
    origins: Mutex<HashSet<String>>,
}

impl ArticleFailures {
    fn blocked(&self, url: &url::Url) -> bool {
        self.origins
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(&url.origin().ascii_serialization())
    }

    /// Returns true when this is the first denial recorded for the source-local origin.
    fn block(&self, url: &url::Url) -> bool {
        self.origins
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(url.origin().ascii_serialization())
    }
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
    if source.content == ContentMode::Light || matches!(source.engine, Engine::Aggr { .. }) {
        return (raw.clone(), fallback);
    }
    let Ok(url) = url::Url::parse(&raw.link) else {
        return (raw.clone(), fallback);
    };
    if failures.blocked(&url) {
        return (raw.clone(), fallback);
    }
    let preview_candidates = raw.preview_candidates.clone();
    let mut page_candidates = preview::HtmlCandidateGroups::default();
    let result = async {
        let cache = crate::cache::ArticleCache::new(cache_dir);
        let headers = http::source_headers(source, &url);
        let cached = cache.load(&url, headers)?;
        let response = client
            .get(http::Request {
                url: &url,
                headers,
                etag: cached.as_ref().and_then(|entry| entry.etag.as_deref()),
                last_modified: cached
                    .as_ref()
                    .and_then(|entry| entry.last_modified.as_deref()),
            })
            .await;
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
        if source.previews {
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
        let extracted = content::extract_article_async(page, response.final_url.clone()).await?;
        cache.store_extracted(&extraction_key, &response.final_url, &extracted)?;
        Ok(extracted)
    }
    .await;
    match result {
        Ok(extracted) => {
            let mut enriched = raw.clone();
            enriched.content_html = Some(extracted.html);
            if source.previews {
                enriched.preview_candidates = preview::ordered_article_candidates(
                    &preview_candidates,
                    page_candidates,
                    extracted.image,
                );
            }
            (enriched, ContentKind::Extracted)
        }
        Err(err) => {
            let denied = matches!(http::status_code(&err), Some(401 | 403 | 429));
            if !denied || failures.block(&url) {
                log::warn!(
                    "{}: heavy content fallback for {}: {err:#}{}",
                    source.slug,
                    raw.link,
                    if denied {
                        "; skipping this origin for the rest of this source's run"
                    } else {
                        ""
                    }
                );
            }
            let mut enriched = raw.clone();
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
    let body = content::strip_leading_metadata(&body, published, &source.slug);
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

    fn source() -> Source {
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
            dry_run: false,
            refresh: false,
            html: true,
            html_max_bytes: 1000,
            article_concurrency: 4,
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

        let test_options = options();
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
        feed.assert_calls_async(1).await;
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
    async fn heavy_stops_retrying_a_host_that_denies_article_requests() {
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
