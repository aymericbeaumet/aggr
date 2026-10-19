//! The private live-reload HTTP server used by `aggr dev`; not intended for production traffic.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse as _, Response, Sse, sse::Event};
use axum::routing::get;
use chrono::Utc;
use futures_util::StreamExt as _;
use notify::{RecursiveMode, Watcher as _};
use tokio::net::TcpListener;
use tokio::sync::{RwLock, broadcast, mpsc, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Duration, sleep};

use super::dev::snapshot::ArchiveSnapshot;
use super::{Project, build, sync};
use crate::cli::{BuildArgs, DevArgs, FetchArgs};
use crate::git::Worktree;

mod vite;

pub(super) fn vite_origin() -> Result<String> {
    Ok(vite::Client::from_env()?
        .map(|client| client.origin().to_owned())
        .unwrap_or_else(|| "http://127.0.0.1:5173".into()))
}

const DEV_KEY_FILE: &str = ".aggr-dev-key";

type SiteFiles = BTreeMap<String, Arc<Vec<u8>>>;

enum Snapshot {
    Pending(Arc<Vec<u8>>),
    Files(SiteFiles),
    Lazy(Arc<crate::site::dev::Site>),
}

#[derive(Clone)]
struct MemorySite {
    snapshot: Arc<RwLock<Snapshot>>,
    revision: Arc<AtomicU64>,
    ownership: Option<Arc<super::lock::DevLock>>,
}

impl MemorySite {
    fn loading() -> Self {
        let page = b"<!doctype html><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>aggr dev</title><style>body{font:16px system-ui;margin:3rem;max-width:40rem}body:before{content:'';display:inline-block;width:.75rem;height:.75rem;margin-right:.6rem;border:2px solid #8ea1ff;border-top-color:transparent;border-radius:50%;animation:s 1s linear infinite}@keyframes s{to{transform:rotate(360deg)}}</style><p>Preparing the archive; pages render on request&hellip;</p>".to_vec();
        let page = Arc::new(page);
        Self {
            ownership: None,
            revision: Arc::default(),
            snapshot: Arc::new(RwLock::new(Snapshot::Pending(page))),
        }
    }

    /// A failed first build must stop looking busy. A usable generation always wins over an
    /// error page, including an earlier successful preview from the same synchronization pass.
    async fn fail_startup(&self, error: &anyhow::Error) -> bool {
        let mut snapshot = self.snapshot.write().await;
        let Snapshot::Pending(previous) = &*snapshot else {
            return false;
        };
        let message = crate::content::escape_html(&format!("{error:#}"));
        let page = format!(
            "<!doctype html><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>aggr dev: preparation failed</title><style>body{{font:16px system-ui;margin:3rem;max-width:40rem}}pre{{white-space:pre-wrap;overflow-wrap:anywhere}}</style><h1>The archive could not be prepared</h1><p>Fix the source or configuration error below. Development will retry when files change or sources refresh.</p><pre>{message}</pre>"
        ).into_bytes();
        if previous.as_slice() == page.as_slice() {
            return false;
        }
        *snapshot = Snapshot::Pending(Arc::new(page));
        self.revision.fetch_add(1, Ordering::SeqCst);
        true
    }

    /// The snapshot the previous `aggr dev` promoted, or nothing when there is none. A snapshot
    /// that cannot be read is removed as well: its dev key would otherwise keep matching, the
    /// first rebuild would be skipped, and the placeholder would be served indefinitely.
    fn cached(root: &Path) -> Option<Self> {
        if !root.join(".aggr-site").is_file() {
            return None;
        }
        match read_site(root, None) {
            Ok(files) => Some(Self {
                ownership: None,
                revision: Arc::default(),
                snapshot: Arc::new(RwLock::new(Snapshot::Files(files))),
            }),
            Err(err) => {
                log::warn!("ignoring unusable dev snapshot {}: {err:#}", root.display());
                if let Err(err) = remove_build(root) {
                    log::warn!("could not remove the unusable dev snapshot: {err:#}");
                }
                None
            }
        }
    }

    #[cfg(test)]
    async fn load(staging: &Path) -> Result<Self> {
        let files = read_site(staging, None)?;
        remove_build(staging)?;
        Ok(Self {
            ownership: None,
            revision: Arc::default(),
            snapshot: Arc::new(RwLock::new(Snapshot::Files(files))),
        })
    }

    async fn replace_from(&self, staging: &Path, cached: &Path) -> Result<()> {
        let ownership = self.ownership.clone();
        let retained = match &*self.snapshot.read().await {
            Snapshot::Files(files) => files.clone(),
            Snapshot::Lazy(_) | Snapshot::Pending(_) => BTreeMap::new(),
        };
        let staging = staging.to_owned();
        let cached = cached.to_owned();
        let files = tokio::task::spawn_blocking(move || -> Result<_> {
            let _ownership = ownership;
            let files = read_site(&staging, Some(&retained))?;
            let previous = cached.with_extension("previous");
            if previous.exists() {
                remove_build(&previous)?;
            }
            if let Some(parent) = cached.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            if cached.exists() {
                std::fs::rename(&cached, &previous).with_context(|| {
                    format!("moving cached dev build {} aside", cached.display())
                })?;
            }
            if let Err(error) = std::fs::rename(&staging, &cached) {
                if previous.exists() {
                    let _ = std::fs::rename(&previous, &cached);
                }
                return Err(error).with_context(|| {
                    format!(
                        "promoting dev build {} to {}",
                        staging.display(),
                        cached.display()
                    )
                });
            }
            if previous.exists() {
                remove_build(&previous)?;
            }
            Ok(files)
        })
        .await
        .context("loading and promoting the dev snapshot")??;
        *self.snapshot.write().await = Snapshot::Files(files);
        self.revision.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn lazy_root(&self) -> Option<PathBuf> {
        match &*self.snapshot.read().await {
            Snapshot::Lazy(site) => Some(site.root().to_owned()),
            Snapshot::Files(_) | Snapshot::Pending(_) => None,
        }
    }

    async fn replace_lazy(&self, site: crate::site::dev::Site) {
        *self.snapshot.write().await = Snapshot::Lazy(Arc::new(site));
        self.revision.fetch_add(1, Ordering::SeqCst);
    }

    async fn body(&self, key: &str) -> Result<Option<Arc<Vec<u8>>>> {
        let site = match &*self.snapshot.read().await {
            Snapshot::Pending(page) => {
                return Ok(matches!(key, "index.html" | "404.html").then(|| Arc::clone(page)));
            }
            Snapshot::Files(files) => return Ok(files.get(key).cloned()),
            Snapshot::Lazy(site) => Arc::clone(site),
        };
        let key = key.to_owned();
        let ownership = self.ownership.clone();
        // The snapshot owns its scratch directory until the last in-flight request ends.
        tokio::task::spawn_blocking(move || {
            let _ownership = ownership;
            site.response(&key).map(|body| body.map(Arc::new))
        })
        .await
        .context("rendering dev response")?
    }

    async fn resolve_response(
        &self,
        base: &str,
        path: &str,
    ) -> Result<Option<(String, Arc<Vec<u8>>)>> {
        let Some(key) = resolve_key(base, path) else {
            return Ok(None);
        };
        Ok(self.body(&key).await?.map(|body| (key, body)))
    }

    #[cfg(test)]
    async fn response(&self, base: &str, path: &str) -> Option<(String, Arc<Vec<u8>>)> {
        self.resolve_response(base, path).await.unwrap()
    }

    async fn not_found(&self) -> (String, Arc<Vec<u8>>) {
        let key = "404.html".to_string();
        let body = self
            .body(&key)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| Arc::new(b"not found".to_vec()));
        (key, body)
    }
}

#[derive(Clone)]
struct DevState {
    data: PathBuf,
    cache: PathBuf,
    cached: PathBuf,
    staging: PathBuf,
    site: MemorySite,
    reload: broadcast::Sender<()>,
    preparation: Arc<crate::site::dev::PreparationCache>,
}

struct DevWatcher {
    task: JoinHandle<()>,
}

impl Drop for DevWatcher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct PendingWatcher {
    watcher: notify::RecommendedWatcher,
    events: mpsc::UnboundedSender<notify::Result<notify::Event>>,
    changes: mpsc::UnboundedReceiver<notify::Result<notify::Event>>,
    paths: WatchPaths,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchPaths {
    configs: Vec<PathBuf>,
    themes: Vec<PathBuf>,
    excluded: Vec<PathBuf>,
    recursive: Vec<PathBuf>,
    shallow: Vec<PathBuf>,
}

fn immutable_site_file(key: &str) -> bool {
    fn hex(value: &str, length: usize) -> bool {
        value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    }
    if let Some(rest) = key.strip_prefix("pagefind/") {
        return rest
            .split_once('/')
            .is_some_and(|(version, file)| hex(version, 64) && !file.is_empty());
    }
    let Some(asset) = key.strip_prefix("assets/") else {
        return false;
    };
    let name = asset.rsplit('/').next().unwrap_or_default();
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    if (asset.starts_with("images/") || asset.starts_with("previews/")) && hex(stem, 40) {
        return true;
    }
    stem.rsplit_once('-')
        .is_some_and(|(_, digest)| hex(digest, 12))
}

fn read_site(root: &Path, previous: Option<&SiteFiles>) -> Result<SiteFiles> {
    let mut files = BTreeMap::new();
    let mut pending = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry.with_context(|| format!("reading {}", root.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(root)?;
        let key = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        // Generated immutable names encode their content; mutable pages and manifests
        // must always be reread even when their length has not changed.
        if immutable_site_file(&key)
            && let Some(body) = previous.and_then(|files| files.get(&key))
            && entry.metadata()?.len() == body.len() as u64
        {
            files.insert(key, Arc::clone(body));
        } else {
            pending.push((key, entry.into_path()));
        }
    }
    if pending.is_empty() {
        return Ok(files);
    }
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(4);
    let chunks = std::thread::scope(|scope| {
        let tasks = pending
            .chunks(pending.len().div_ceil(workers))
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|(key, path)| {
                            let body = std::fs::read(path)
                                .with_context(|| format!("reading {}", path.display()))?;
                            Ok((key.clone(), Arc::new(body)))
                        })
                        .collect::<Result<SiteFiles>>()
                })
            })
            .collect::<Vec<_>>();
        tasks
            .into_iter()
            .map(|task| {
                task.join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("dev snapshot reader panicked")))
            })
            .collect::<Vec<_>>()
    });
    for chunk in chunks {
        files.extend(chunk?);
    }
    Ok(files)
}

fn remove_build(root: &Path) -> Result<()> {
    if root.exists() {
        std::fs::remove_dir_all(root)
            .with_context(|| format!("removing transient build {}", root.display()))?;
    }
    Ok(())
}

pub(super) async fn run_with_reload(
    project: Project,
    args: &DevArgs,
    data: PathBuf,
    cache: PathBuf,
    cached: PathBuf,
    staging: PathBuf,
    ownership: Arc<super::lock::DevLock>,
) -> Result<()> {
    let vite = vite::Client::from_env()?;
    let project = Arc::new(project);
    // Install the process signal handler before the listener becomes visible. This makes even an
    // immediate Ctrl-C (common when a command was started by mistake) a graceful shutdown.
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let shutdown_task = tokio::spawn(async move {
        let _ = shutdown_tx.send(tokio::signal::ctrl_c().await);
    });
    tokio::task::yield_now().await;
    let build_args = args.build_args();
    let base = crate::site::base_path(build::base_url(&project, &build_args)?.as_deref());
    let listener = bind(args.port).await?;
    let url = format!("http://127.0.0.1:{}{base}", listener.local_addr()?.port());
    println!("aggr dev: {url}");
    println!(
        "syncing {} source(s) in the background…",
        project.sources.len()
    );
    let mut site = match args.release.then(|| MemorySite::cached(&cached)).flatten() {
        Some(site) => {
            println!("restored the previous dev build from cache");
            site
        }
        None => MemorySite::loading(),
    };
    site.ownership = Some(ownership);
    let (reload, _) = broadcast::channel(16);
    let (stop_http, http_shutdown) = watch::channel(false);
    let serving = host(
        listener,
        site.clone(),
        &base,
        reload.clone(),
        vite,
        http_shutdown,
    );
    tokio::pin!(serving);
    let state = DevState {
        data,
        cache,
        cached,
        staging,
        site,
        reload,
        preparation: Arc::default(),
    };
    // Register filesystem watches before the first network sync/build. Editors can otherwise save
    // a config or theme while startup is busy and leave the served snapshot stale until the next
    // manual edit.
    let pending_watcher = prepare_watch(&project, &state)?;
    let initializing = async {
        let data = state.data.clone();
        let has_cached_items = tokio::task::spawn_blocking(move || {
            crate::store::Store::open(data)
                .items()
                .map(|items| !items.is_empty())
        })
        .await
        .context("reading cached dev articles")??;
        if has_cached_items {
            match refresh(project.clone(), &args.fetch, &build_args, &state, false).await {
                Ok(_) => println!("cached articles ready: {url}"),
                Err(err) => eprintln!("cached build failed: {err:#}"),
            }
        }
        let project = match Project::load(&project.config_path).await {
            Ok(resolved) => {
                let resolved = Arc::new(resolved);
                match refresh(resolved.clone(), &args.fetch, &build_args, &state, true).await {
                    Ok(_) => println!("ready: {url}"),
                    Err(err) => eprintln!("initial build failed: {err:#}"),
                }
                resolved
            }
            Err(err) => {
                eprintln!("initial source resolution failed: {err:#}");
                if state.site.fail_startup(&err).await {
                    let _ = state.reload.send(());
                }
                project.clone()
            }
        };
        let _watcher = pending_watcher.start(project, args, state)?;
        std::future::pending::<Result<()>>().await
    };
    let (result, http_finished) = tokio::select! {
        result = &mut serving => (result, true),
        result = initializing => (result, false),
        signal = &mut shutdown_rx => {
            let result = signal.context("Ctrl-C task stopped")
                .and_then(|signal| signal.context("listening for Ctrl-C"));
            if result.is_ok() {
                println!("stopping");
            }
            (result, false)
        }
    };
    stop_http.send_replace(true);
    let result = if http_finished {
        result
    } else {
        result.and(serving.await)
    };
    shutdown_task.abort();
    result
}

async fn refresh(
    project: Arc<Project>,
    fetch_args: &FetchArgs,
    build_args: &BuildArgs,
    state: &DevState,
    sync_sources: bool,
) -> Result<bool> {
    let result = refresh_inner(project, fetch_args, build_args, state, sync_sources).await;
    if let Err(error) = &result
        && state.site.fail_startup(error).await
    {
        let _ = state.reload.send(());
    }
    result
}

async fn refresh_inner(
    project: Arc<Project>,
    fetch_args: &FetchArgs,
    build_args: &BuildArgs,
    state: &DevState,
    sync_sources: bool,
) -> Result<bool> {
    let worktree = Worktree::ephemeral(state.data.clone());
    let visible_change = if sync_sources {
        let report = if build_args.release {
            sync::run_dev(
                &project,
                &worktree,
                fetch_args,
                &state.cache,
                state.site.ownership.clone(),
            )
            .await?
        } else {
            sync_with_previews(
                project.clone(),
                worktree,
                fetch_args.clone(),
                build_args,
                state,
            )
            .await?
        };
        println!(
            "dev data: {} new item(s) in the isolated system cache (never committed or pushed)",
            report.added()
        );
        report.added() > 0
            || report.reprocessed > 0
            || report.removed > 0
            || report.status_changed
            || report
                .sources
                .iter()
                .any(|source| source.outcome == crate::store::Outcome::Ok && !source.unchanged)
    } else {
        println!("rebuilding from cached source data");
        false
    };
    let store = crate::store::Store::open(&state.data);
    let discussions = crate::discussions::cached(
        &project.config.defaults.discussions,
        &crate::discussions::by_source(&project.sources),
        &store.items()?,
        &state.cache,
    );
    if sync_sources {
        render_with_discussion_refresh(
            project.clone(),
            build_args,
            state,
            discussions,
            visible_change,
            build::resolve_discussions(&project, &store, &state.cache, Utc::now()),
        )
        .await
    } else {
        render_snapshot(project, build_args, state, discussions, visible_change).await
    }
}

async fn capture_archive(state: &DevState) -> Result<Arc<ArchiveSnapshot>> {
    let data = state.data.clone();
    let ownership = state.site.ownership.clone();
    let scratch = state.staging.with_extension("archives");
    tokio::task::spawn_blocking(move || {
        let _ownership = ownership;
        std::fs::create_dir_all(&scratch).context("creating dev archive staging directory")?;
        ArchiveSnapshot::capture(&data.canonicalize()?, &scratch.canonicalize()?).map(Arc::new)
    })
    .await
    .context("capturing stable dev archive")?
}

async fn sync_with_previews(
    project: Arc<Project>,
    worktree: Worktree,
    fetch_args: FetchArgs,
    build_args: &BuildArgs,
    state: &DevState,
) -> Result<super::fetch::Report> {
    let mut archive = capture_archive(state).await?;
    let (completed, mut sources) = mpsc::channel(1);
    let mut syncing = JoinSet::new();
    let fetch_project = project.clone();
    let cache = state.cache.clone();
    let ownership = state.site.ownership.clone();
    syncing.spawn(async move {
        sync::run_dev_with_progress(
            &fetch_project,
            &worktree,
            &fetch_args,
            &cache,
            completed,
            ownership,
        )
        .await
    });
    let mut pending = None;
    let mut updates = tokio::time::interval(Duration::from_millis(250));
    loop {
        let event = tokio::select! {
            result = syncing.join_next() => {
                if let Some(archive) = pending.take() {
                    publish_preview(&project, build_args, state, archive).await;
                }
                return result.context("dev sync worker stopped")?.context("dev sync worker panicked")?;
            }
            Some(event) = sources.recv() => event,
            _ = updates.tick(), if pending.is_some() => {
                if let Some(archive) = pending.take() {
                    publish_preview(&project, build_args, state, archive).await;
                }
                continue;
            }
        };
        let updated = match event {
            super::fetch::Progress::Prepared(resume) => {
                let captured = capture_archive(state).await;
                let _ = resume.send(());
                captured
            }
            super::fetch::Progress::Source { slug, captured } => {
                let previous = Arc::clone(&archive);
                let data = state.data.canonicalize()?;
                let ownership = state.site.ownership.clone();
                let updated = tokio::task::spawn_blocking(move || {
                    let _ownership = ownership;
                    previous.completed(&data, &slug).map(Arc::new)
                })
                .await
                .context("capturing completed dev source")?;
                let _ = captured.send(());
                updated
            }
        };
        match updated {
            Ok(snapshot) => {
                archive = snapshot;
                pending = Some(Arc::clone(&archive));
            }
            Err(error) => {
                log::warn!("dev preview update: {error:#}");
                continue;
            }
        }
    }
}

async fn publish_preview(
    project: &Arc<Project>,
    build_args: &BuildArgs,
    state: &DevState,
    archive: Arc<ArchiveSnapshot>,
) {
    let result = async {
        let discussions = crate::discussions::cached(
            &project.config.defaults.discussions,
            &crate::discussions::by_source(&project.sources),
            &crate::store::Store::open(archive.root()).items()?,
            &state.cache,
        );
        render_snapshot_from_archive(
            project.clone(),
            build_args,
            state,
            discussions,
            true,
            Some(archive),
        )
        .await
    }
    .await;
    if let Err(error) = result {
        log::warn!("dev preview render: {error:#}");
    }
}

async fn render_with_discussion_refresh(
    project: Arc<Project>,
    build_args: &BuildArgs,
    state: &DevState,
    cached: crate::discussions::ResolutionSet,
    visible_change: bool,
    fresh: impl std::future::Future<Output = Result<crate::discussions::ResolutionSet>>,
) -> Result<bool> {
    let previous = cached.fingerprint();
    let rebuilt =
        render_snapshot(project.clone(), build_args, state, cached, visible_change).await?;
    if !crate::discussions::any_provider(&project.config.defaults.discussions, &project.sources) {
        return Ok(rebuilt);
    }
    println!("articles ready; refreshing discussion links in the background…");
    let discussions = match fresh.await {
        Ok(discussions) => discussions,
        Err(error) => {
            log::warn!("discussion refresh: {error:#}");
            return Ok(rebuilt);
        }
    };
    if discussions.fingerprint() == previous {
        return Ok(rebuilt);
    }
    let enriched = render_snapshot(project, build_args, state, discussions, false).await?;
    Ok(rebuilt || enriched)
}

async fn render_snapshot(
    project: Arc<Project>,
    build_args: &BuildArgs,
    state: &DevState,
    discussions: crate::discussions::ResolutionSet,
    visible_change: bool,
) -> Result<bool> {
    let archive = if build_args.release {
        None
    } else {
        Some(capture_archive(state).await?)
    };
    render_snapshot_from_archive(
        project,
        build_args,
        state,
        discussions,
        visible_change,
        archive,
    )
    .await
}

async fn render_snapshot_from_archive(
    project: Arc<Project>,
    build_args: &BuildArgs,
    state: &DevState,
    discussions: crate::discussions::ResolutionSet,
    visible_change: bool,
    archive: Option<Arc<ArchiveSnapshot>>,
) -> Result<bool> {
    let base_url = build::base_url(&project, build_args)?;
    let now = Utc::now();
    let build_args = build_args.clone();
    let data = archive
        .as_ref()
        .map_or_else(|| state.data.clone(), |archive| archive.root().to_owned());
    let cache = state.cache.clone();
    let cached = if build_args.release {
        state.cached.clone()
    } else {
        state.site.lazy_root().await.unwrap_or_default()
    };
    let staging = state.staging.clone();
    let preparation = Arc::clone(&state.preparation);
    let ownership = state.site.ownership.clone();
    let (rendered, finished) = oneshot::channel();
    // A dedicated thread keeps template rendering from blocking the signal future. Unlike
    // `spawn_blocking`, it also cannot make Tokio wait for a long render while shutting down;
    // interrupted output stays in the disposable staging directory and is cleared next run.
    std::thread::Builder::new()
        .name("aggr-dev-render".to_string())
        .spawn(move || {
            let _ownership = ownership;
            let result = render_refresh(
                &project,
                &build_args,
                &data,
                &cache,
                &cached,
                &staging,
                base_url,
                discussions,
                now,
                visible_change,
                &preparation,
            );
            let result = result.map(|rendered| match (rendered, archive) {
                (RenderedSnapshot::Lazy(site), Some(archive)) => {
                    RenderedSnapshot::Lazy(Box::new((*site).own_archive(archive.directory())))
                }
                (rendered, _) => rendered,
            });
            let _ = rendered.send(result);
        })
        .context("starting dev render worker")?;
    let rendered = finished
        .await
        .context("dev render worker stopped before finishing")??;
    match rendered {
        RenderedSnapshot::Current => return Ok(false),
        RenderedSnapshot::Release => {
            state
                .site
                .replace_from(&state.staging, &state.cached)
                .await?;
        }
        RenderedSnapshot::Lazy(site) => state.site.replace_lazy(*site).await,
    }
    let _ = state.reload.send(());
    Ok(true)
}

enum RenderedSnapshot {
    Current,
    Release,
    Lazy(Box<crate::site::dev::Site>),
}

#[allow(clippy::too_many_arguments)]
fn render_refresh(
    project: &Project,
    build_args: &BuildArgs,
    data: &Path,
    cache: &Path,
    cached: &Path,
    staging: &Path,
    base_url: Option<String>,
    discussions: crate::discussions::ResolutionSet,
    now: chrono::DateTime<Utc>,
    visible_change: bool,
    preparation: &crate::site::dev::PreparationCache,
) -> Result<RenderedSnapshot> {
    let store = crate::store::Store::open(data);
    let generation = crate::site::render_generation(
        &store.retained_items(&project.sources, now)?,
        &project.config.site,
        now,
    );
    let discussions_fingerprint = discussions.fingerprint();
    let config_sha = project.config_sha();
    let fingerprint = crate::cache::render_fingerprint(crate::cache::RenderFingerprint {
        config: &project.config,
        theme: &crate::site::theme(),
        repo_root: project.repo.root(),
        config_sha: config_sha.as_deref(),
        data_sha: None,
        base_url: base_url.as_deref(),
        release: build_args.release,
        discussions: Some(&discussions_fingerprint),
        generation: &generation,
    })?;
    if !rebuild_required(visible_change, cached, &fingerprint) {
        println!("dev build already current");
        return Ok(RenderedSnapshot::Current);
    }
    if !build_args.release {
        let scratch = staging
            .parent()
            .context("dev staging directory has no parent")?;
        std::fs::create_dir_all(scratch).context("creating dev snapshot staging directory")?;
        let directory = tempfile::Builder::new()
            .prefix("snapshot-")
            .tempdir_in(scratch)
            .context("creating isolated dev snapshot")?;
        let out = directory.path().join("site");
        let info = crate::site::BuildInfo {
            hermetic: false,
            metrics: Default::default(),
            out: out.clone(),
            base_url,
            config_sha,
            config_path: project.config_repo_path(),
            data_sha: None,
            generation,
            now,
            release: false,
            discussions,
            development: true,
            render_cache_key: None,
            pagefind_cache: Some(cache.to_owned()),
        };
        let site = crate::site::dev::Site::prepare(
            &project.config,
            &project.sources,
            store.with_image_cache(cache),
            &project.root,
            info,
            preparation,
        )?
        .own_directory(directory);
        crate::cache::write(&out.join(DEV_KEY_FILE), fingerprint.as_bytes())
            .context("writing the dev snapshot fingerprint")?;
        println!("prepared dev snapshot; pages and media render on request");
        return Ok(RenderedSnapshot::Lazy(Box::new(site)));
    }
    // The staging directory is disposable. A template error may leave a partial tree without the
    // safety marker used for user-selected output directories, so always reset it before retrying.
    remove_build(staging)?;
    build::run_ephemeral(project, build_args, data, staging, cache, discussions)?;
    crate::cache::write(&staging.join(DEV_KEY_FILE), fingerprint.as_bytes())
        .context("writing the dev build fingerprint")?;
    Ok(RenderedSnapshot::Release)
}

fn rebuild_required(visible_change: bool, site: &Path, fingerprint: &str) -> bool {
    visible_change || site.as_os_str().is_empty() || !dev_key_matches(site, fingerprint)
}

fn dev_key_matches(site: &Path, fingerprint: &str) -> bool {
    std::fs::read_to_string(site.join(DEV_KEY_FILE))
        .ok()
        .is_some_and(|stored| stored == fingerprint)
        && site.join(".aggr-site").is_file()
}

async fn host(
    listener: TcpListener,
    site: MemorySite,
    base: &str,
    reload: broadcast::Sender<()>,
    vite: Option<vite::Client>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let state = Arc::new(HttpState {
        site,
        base: base.to_owned(),
        run: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_string(),
        reload,
        vite,
        shutdown: shutdown.clone(),
    });
    axum::serve(
        listener,
        Router::new().fallback(get(handle)).with_state(state),
    )
    .with_graceful_shutdown(async move {
        let _ = shutdown.wait_for(|stopping| *stopping).await;
    })
    .await
    .context("serving development HTTP")
}

/// Prefer the stable dev port, but never make iteration fail because another project owns it.
async fn bind(port: u16) -> Result<TcpListener> {
    match TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => Ok(listener),
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            log::warn!("127.0.0.1:{port} is busy; choosing a free port");
            TcpListener::bind(("127.0.0.1", 0))
                .await
                .context("binding an available localhost port")
        }
        Err(err) => Err(err).with_context(|| format!("binding 127.0.0.1:{port}")),
    }
}

fn prepare_watch(project: &Project, state: &DevState) -> Result<PendingWatcher> {
    let (events, changes) = mpsc::unbounded_channel();
    let paths = WatchPaths::new(project, state)?;
    let watcher = create_watcher(&events, &paths)?;
    Ok(PendingWatcher {
        watcher,
        events,
        changes,
        paths,
    })
}

impl PendingWatcher {
    fn start(self, project: Arc<Project>, args: &DevArgs, state: DevState) -> Result<DevWatcher> {
        let Self {
            watcher,
            events,
            mut changes,
            paths,
        } = self;
        let next_paths = WatchPaths::new(&project, &state)?;
        let watcher = if next_paths != paths {
            create_watcher(&events, &next_paths)?
        } else {
            watcher
        };
        let build_args = args.build_args();
        let fetch_args = args.fetch.clone();
        let task = tokio::spawn(async move {
            // Kept in the task so aborting it synchronously drops the native watcher too.
            let mut active_watcher = watcher;
            let mut active_paths = next_paths;
            let mut active_project = project;
            while let Some(event) = changes.recv().await {
                let Ok(event) = event else { continue };
                if !rebuild_event(event.kind) {
                    continue;
                }
                let mut changed = event
                    .paths
                    .into_iter()
                    .filter(|path| active_paths.watched(path))
                    .collect::<Vec<_>>();
                if changed.is_empty() {
                    continue;
                }
                sleep(Duration::from_millis(150)).await;
                while let Ok(event) = changes.try_recv() {
                    if let Ok(event) = event
                        && rebuild_event(event.kind)
                    {
                        changed.extend(
                            event
                                .paths
                                .into_iter()
                                .filter(|path| active_paths.watched(path)),
                        );
                    }
                }
                changed.sort();
                changed.dedup();
                let sync_sources = changed
                    .iter()
                    .any(|path| active_paths.configs.contains(path));
                let result: Result<bool> = async {
                    let project = reload_project(active_project.clone(), sync_sources).await?;
                    let next_paths = WatchPaths::new(&project, &state)?;
                    if next_paths != active_paths {
                        let next_watcher = create_watcher(&events, &next_paths)?;
                        active_watcher = next_watcher;
                        active_paths = next_paths;
                    }
                    active_project = project.clone();
                    refresh(project, &fetch_args, &build_args, &state, sync_sources).await
                }
                .await;
                match result {
                    Ok(true) => println!("reloaded"),
                    Ok(false) => println!("already current"),
                    Err(err) => eprintln!("build failed: {err:#}"),
                }
            }
            drop(active_watcher);
        });
        Ok(DevWatcher { task })
    }
}

async fn reload_project(project: Arc<Project>, config_changed: bool) -> Result<Arc<Project>> {
    if config_changed {
        Ok(Arc::new(Project::load(&project.config_path).await?))
    } else {
        Ok(project)
    }
}

impl WatchPaths {
    fn new(project: &Project, state: &DevState) -> Result<Self> {
        let mut configs = project.config.loaded_files.clone();
        configs.sort();
        configs.dedup();

        // A development binary renders the theme from its source tree, so an edit there rebuilds
        // without recompiling; a release binary has no theme files to watch.
        let themes = crate::site::theme()
            .source_dir()
            .map(|source| normalize_watch_path(source.to_path_buf()))
            .transpose()?
            .into_iter()
            .collect::<Vec<_>>();

        let mut excluded = vec![
            state.data.clone(),
            state.cache.clone(),
            state.cached.clone(),
            state.cached.with_extension("previous"),
            state.staging.clone(),
        ];
        if let Some(scratch) = state.staging.parent() {
            excluded.push(scratch.to_owned());
        }
        excluded.sort();
        excluded.dedup();

        let mut recursive = themes
            .iter()
            .filter(|path| path.is_dir())
            .cloned()
            .collect::<Vec<_>>();
        recursive.sort();
        recursive.dedup();

        let mut shallow = configs
            .iter()
            .filter_map(|path| path.parent().map(Path::to_path_buf))
            .collect::<Vec<_>>();
        shallow.retain(|path| !recursive.iter().any(|root| path.starts_with(root)));
        shallow.sort();
        shallow.dedup();

        Ok(Self {
            configs,
            themes,
            excluded,
            recursive,
            shallow,
        })
    }

    fn watched(&self, path: &Path) -> bool {
        watched(path, &self.configs, &self.themes, &self.excluded)
    }
}

fn create_watcher(
    events: &mpsc::UnboundedSender<notify::Result<notify::Event>>,
    paths: &WatchPaths,
) -> Result<notify::RecommendedWatcher> {
    let events = events.clone();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = events.send(event);
    })?;
    for root in &paths.recursive {
        watcher
            .watch(root, RecursiveMode::Recursive)
            .with_context(|| format!("watching {}", root.display()))?;
        log::info!("watching {}", root.display());
    }
    for root in &paths.shallow {
        watcher
            .watch(root, RecursiveMode::NonRecursive)
            .with_context(|| format!("watching {}", root.display()))?;
        log::info!("watching {}", root.display());
    }
    Ok(watcher)
}

fn normalize_watch_path(path: PathBuf) -> Result<PathBuf> {
    if path.exists() {
        return path
            .canonicalize()
            .with_context(|| format!("resolving watch path {}", path.display()));
    }
    let mut normalized = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component),
        }
    }
    Ok(normalized)
}

fn rebuild_event(kind: notify::EventKind) -> bool {
    !matches!(
        kind,
        notify::EventKind::Access(_)
            | notify::EventKind::Modify(notify::event::ModifyKind::Metadata(
                notify::event::MetadataKind::AccessTime
            ))
    )
}

fn watched(path: &Path, configs: &[PathBuf], themes: &[PathBuf], excluded: &[PathBuf]) -> bool {
    (configs.iter().any(|config| path == config)
        || themes
            .iter()
            .any(|theme| path.starts_with(theme) || theme.starts_with(path)))
        && !excluded.iter().any(|root| path.starts_with(root))
        && !path
            .components()
            .any(|part| matches!(part.as_os_str().to_str(), Some(".git" | ".aggr" | "target")))
}

struct HttpState {
    site: MemorySite,
    base: String,
    run: String,
    reload: broadcast::Sender<()>,
    vite: Option<vite::Client>,
    shutdown: watch::Receiver<bool>,
}

async fn handle(State(state): State<Arc<HttpState>>, uri: Uri) -> Response {
    let path = uri.path();
    if path == format!("{}__aggr/reload", state.base) {
        // Subscribe before comparing generations so a concurrent promotion cannot be missed.
        let updates = state.reload.subscribe();
        let previous_run = url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
            .any(|(key, value)| match key.as_ref() {
                "run" => value != state.run,
                "revision" => value
                    .parse::<u64>()
                    .ok()
                    .is_some_and(|revision| revision != state.site.revision.load(Ordering::SeqCst)),
                _ => false,
            });
        let mut shutdown = state.shutdown.clone();
        let events = futures_util::stream::unfold(
            (updates, previous_run),
            |(mut updates, reconnect)| async move {
                if !reconnect
                    && matches!(
                        updates.recv().await,
                        Err(broadcast::error::RecvError::Closed)
                    )
                {
                    return None;
                }
                Some((
                    Ok::<_, std::convert::Infallible>(Event::default().data("reload")),
                    (updates, false),
                ))
            },
        )
        .take_until(async move {
            let _ = shutdown.wait_for(|stopping| *stopping).await;
        });
        return (
            [(header::CACHE_CONTROL, "no-store")],
            Sse::new(events).keep_alive(axum::response::sse::KeepAlive::default()),
        )
            .into_response();
    }
    // An older revision on a newer body causes a harmless retry; the reverse could miss an update.
    let revision = state.site.revision.load(Ordering::SeqCst);
    let (status, key, mut body) = match state.site.resolve_response(&state.base, path).await {
        Ok(Some((file, body))) => (StatusCode::OK, file, body),
        Ok(None) => {
            let (file, body) = state.site.not_found().await;
            (StatusCode::NOT_FOUND, file, body)
        }
        Err(error) => {
            log::error!("could not render dev route {path}: {error:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "error.txt".to_string(),
                Arc::new(b"Could not render this page. See the development server log.".to_vec()),
            )
        }
    };
    let file = Path::new(&key);
    if file
        .extension()
        .is_some_and(|extension| extension == "html")
    {
        if let Some(client) = &state.vite {
            body = Arc::new(client.inject(&body));
        }
        body = Arc::new(inject_reload(&body, &state.base, &state.run, revision));
    }
    let mut response = Response::new(Body::from(bytes::Bytes::from_owner(SharedBody(body))));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        content_type(file).parse().expect("static content type"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        cache_control(&key).parse().expect("static cache policy"),
    );
    if file
        .extension()
        .is_some_and(|extension| extension == "html")
    {
        headers.insert(header::PRAGMA, "no-cache".parse().expect("static pragma"));
        headers.insert(header::EXPIRES, "0".parse().expect("static expiry"));
    }
    response
}

struct SharedBody(Arc<Vec<u8>>);

impl AsRef<[u8]> for SharedBody {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

fn inject_reload(body: &[u8], base: &str, run: &str, revision: u64) -> Vec<u8> {
    let script = format!(
        "<script>(function(){{var b=new URL({0:?},location.origin),u=new URL(location.href);if(u.searchParams.delete('__aggr_dev'))history.replaceState(null,'',u);if('serviceWorker'in navigator)navigator.serviceWorker.getRegistration(b).then(function(r){{if(r)r.unregister()}});if('caches'in window){{var p='aggr:'+encodeURIComponent(b.pathname)+':';caches.keys().then(function(k){{k.filter(function(x){{return x.indexOf(p)===0}}).forEach(function(x){{caches.delete(x)}})}})}}new EventSource('{0}__aggr/reload?run={1}&revision={2}').onmessage=function(){{var n=new URL(location.href);n.searchParams.set('__aggr_dev',Date.now());location.replace(n)}}}})();</script>",
        base, run, revision
    );
    let html = String::from_utf8_lossy(body);
    if let Some(at) = html.rfind("</body>") {
        format!("{}{}{}", &html[..at], script, &html[at..]).into_bytes()
    } else {
        format!("{html}{script}").into_bytes()
    }
}

/// Map a request path to a file under `root`, honoring the base path a release build was made
/// for, serving `index.html` for directories, and refusing anything that escapes the root.
fn resolve_key(base: &str, path: &str) -> Option<String> {
    let decoded = percent_decode(path);
    let rest = decoded.strip_prefix(base.trim_end_matches('/'))?;
    if !rest.is_empty() && !rest.starts_with('/') {
        return None;
    }
    let mut parts = Vec::new();
    for segment in rest.split('/').filter(|s| !s.is_empty()) {
        match Path::new(segment).components().next() {
            Some(Component::Normal(_)) => parts.push(segment),
            _ => return None,
        }
    }
    if rest.is_empty() || rest.ends_with('/') {
        parts.push("index.html");
    }
    Some(parts.join("/"))
}

#[cfg(test)]
pub fn resolve(root: &Path, base: &str, path: &str) -> Option<PathBuf> {
    let file = root.join(resolve_key(base, path)?);
    file.is_file().then_some(file)
}

fn percent_decode(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

const PAGE_CACHE: &str = "no-store, no-cache, must-revalidate, max-age=0";
const ASSET_CACHE: &str = "public, max-age=31536000, immutable";
const FRESH_CACHE: &str = "no-store";

/// How one development response may be stored. HTML stays uncached so a reload shows the
/// snapshot just written, without `Clear-Site-Data`: that header dumps the HTTP cache on every
/// document, and the reload script already drops the reader's own caches. A content-addressed
/// name keeps its bytes for a year. The shapes match `immutable` in `web/src/sw/routing.ts`.
fn cache_control(key: &str) -> &'static str {
    if Path::new(key)
        .extension()
        .is_some_and(|extension| extension == "html")
    {
        return PAGE_CACHE;
    }
    if immutable_asset(key) {
        ASSET_CACHE
    } else {
        FRESH_CACHE
    }
}

fn immutable_asset(key: &str) -> bool {
    if let Some(rest) = key.strip_prefix("pagefind/") {
        let id = rest.split('/').next().unwrap_or("");
        return rest.contains('/') && is_lower_hex(id, 64);
    }
    let Some(asset) = key.strip_prefix("assets/") else {
        return false;
    };
    let name = asset.rsplit('/').next().unwrap_or("");
    let stem = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);
    if (asset.starts_with("images/")
        || asset.starts_with("previews/")
        || asset.starts_with("documents/"))
        && is_lower_hex(stem, 40)
    {
        return true;
    }
    if asset.starts_with("app/")
        && (name.ends_with(".js") || name.ends_with(".css"))
        && ends_with_vite_hash(stem)
    {
        return true;
    }
    hashed_suffix(stem, 12, |byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn hashed_suffix(stem: &str, len: usize, accept: impl Fn(u8) -> bool) -> bool {
    let bytes = stem.as_bytes();
    bytes.len() > len
        && bytes[bytes.len() - len - 1] == b'-'
        && bytes[bytes.len() - len..].iter().copied().all(accept)
}

fn ends_with_vite_hash(stem: &str) -> bool {
    hashed_suffix(stem, 8, |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
    })
}

pub fn content_type(path: &Path) -> &'static str {
    match path.file_name().and_then(|name| name.to_str()) {
        Some("atom.xml" | "feed.xml") => return "application/atom+xml; charset=utf-8",
        Some("rss.xml") => return "application/rss+xml; charset=utf-8",
        Some("feed.json") => return "application/feed+json; charset=utf-8",
        Some("opensearch.xml") => {
            return "application/opensearchdescription+xml; charset=utf-8";
        }
        _ => {}
    }
    if path
        .extension()
        .is_some_and(|extension| extension == "json")
        && path
            .components()
            .any(|component| component.as_os_str() == "items")
    {
        return "application/ld+json; charset=utf-8";
    }
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("pdf") => "application/pdf",
        Some("webmanifest") => "application/manifest+json",
        Some("xml") => "application/xml",
        Some("md") => "text/markdown; charset=utf-8",
        Some("rst") => "text/x-rst; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        Some("toml") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn cold_failure_shows_an_escaped_retry_page_and_preserves_later_success() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .arg(root.path())
                .status()
                .unwrap()
                .success()
        );
        let config = root.path().join("aggr.toml");
        std::fs::write(&config, "[defaults]\ndiscussions=[]\n").unwrap();
        let project = Arc::new(Project::load_offline(&config).await.unwrap());
        let (reload, mut changes) = broadcast::channel(16);
        let state = DevState {
            data: root.path().join("missing-archive"),
            cache: root.path().join("cache"),
            cached: root.path().join("cached"),
            staging: root.path().join("scratch/site"),
            site: MemorySite::loading(),
            reload,
            preparation: Arc::default(),
        };
        assert!(
            refresh(
                project.clone(),
                &FetchArgs::default(),
                &BuildArgs::default(),
                &state,
                false
            )
            .await
            .is_err()
        );
        changes.try_recv().unwrap();
        let body = state.site.body("index.html").await.unwrap().unwrap();
        assert!(String::from_utf8_lossy(&body).contains("The archive could not be prepared"));
        assert!(!String::from_utf8_lossy(&body).contains("Preparing the archive"));

        let error = anyhow::anyhow!("upstream <script>alert('oops')</script> & retry");
        assert!(state.site.fail_startup(&error).await);
        let revision = state.site.revision.load(Ordering::SeqCst);
        assert!(!state.site.fail_startup(&error).await);
        assert_eq!(state.site.revision.load(Ordering::SeqCst), revision);
        let response = handle(
            State(Arc::new(HttpState {
                site: state.site.clone(),
                base: "/".into(),
                run: "test".into(),
                reload: state.reload.clone(),
                vite: None,
                shutdown: watch::channel(false).1,
            })),
            "/".parse().unwrap(),
        )
        .await;
        let html = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8_lossy(&html);
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&amp; retry"));
        assert!(!html.contains("<script>alert"));
        assert!(
            html.contains("new EventSource("),
            "failed startup keeps the recovery bridge"
        );

        let staging = root.path().join("recovered");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("index.html"), "<body>Recovered reader</body>").unwrap();
        std::fs::write(staging.join(".aggr-site"), "fixture").unwrap();
        state
            .site
            .replace_from(&staging, &state.cached)
            .await
            .unwrap();
        assert!(
            refresh(
                project,
                &FetchArgs::default(),
                &BuildArgs::default(),
                &state,
                false
            )
            .await
            .is_err()
        );
        let body = state.site.body("index.html").await.unwrap().unwrap();
        assert_eq!(body.as_slice(), b"<body>Recovered reader</body>");
        assert!(
            changes.try_recv().is_err(),
            "a failed refresh never replaces a good generation"
        );
    }

    async fn test_http_server() -> (
        String,
        broadcast::Sender<()>,
        watch::Sender<bool>,
        JoinHandle<Result<()>>,
    ) {
        crate::http::install_crypto_provider();
        let listener = bind(0).await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let site = MemorySite {
            ownership: None,
            revision: Arc::default(),
            snapshot: Arc::new(RwLock::new(Snapshot::Files(BTreeMap::from([
                (
                    "index.html".into(),
                    Arc::new(b"<body>reader</body>".to_vec()),
                ),
                (
                    "404.html".into(),
                    Arc::new(b"<body>missing</body>".to_vec()),
                ),
                (
                    "assets/app/app-abcdefgh.js".into(),
                    Arc::new(b"export {}".to_vec()),
                ),
                ("paper.pdf".into(), Arc::new(b"%PDF-fixture".to_vec())),
                (
                    "sitory/index.html".into(),
                    Arc::new(b"outside base".to_vec()),
                ),
            ])))),
        };
        let (reload, _) = broadcast::channel(16);
        let (shutdown, stopped) = watch::channel(false);
        let sender = reload.clone();
        let task =
            tokio::spawn(
                async move { host(listener, site, "/repo/", sender, None, stopped).await },
            );
        (origin, reload, shutdown, task)
    }

    #[tokio::test]
    async fn http_contract_preserves_documents_assets_and_head() {
        let (origin, _, _shutdown, task) = test_http_server().await;
        let client = reqwest::Client::new();
        let page = client.get(format!("{origin}/repo/")).send().await.unwrap();
        assert_eq!(page.status(), 200);
        assert_eq!(page.headers()["content-type"], "text/html; charset=utf-8");
        assert!(
            page.headers()["cache-control"]
                .to_str()
                .unwrap()
                .contains("no-store")
        );
        let body = page.bytes().await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("/repo/__aggr/reload"));
        let head = client.head(format!("{origin}/repo/")).send().await.unwrap();
        assert_eq!(head.status(), 200);
        assert_eq!(head.headers()["content-length"], body.len().to_string());
        assert!(head.bytes().await.unwrap().is_empty());
        let asset = client
            .get(format!("{origin}/repo/assets/app/app-abcdefgh.js"))
            .send()
            .await
            .unwrap();
        assert_eq!(
            asset.headers()["cache-control"],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(asset.text().await.unwrap(), "export {}");
        let pdf = client
            .get(format!("{origin}/repo/paper.pdf"))
            .send()
            .await
            .unwrap();
        assert_eq!(pdf.headers()["content-type"], "application/pdf");
        for path in ["/repo/missing", "/repository/", "/repo/%2e%2e%2fpaper.pdf"] {
            let response = client.get(format!("{origin}{path}")).send().await.unwrap();
            assert_eq!(response.status(), 404, "{path}");
        }
        assert_eq!(
            client
                .post(format!("{origin}/repo/"))
                .send()
                .await
                .unwrap()
                .status(),
            405
        );
        task.abort();
    }

    #[tokio::test]
    async fn reload_stream_notifies_changes_and_reconnected_old_processes() {
        use futures_util::StreamExt as _;
        let (origin, reload, _shutdown, task) = test_http_server().await;
        let client = reqwest::Client::new();
        let response = client
            .get(format!("{origin}/repo/__aggr/reload"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        let mut stream = response.bytes_stream();
        reload.send(()).unwrap();
        let event = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&event).contains("data: reload"));
        drop(stream);
        let response = client
            .get(format!("{origin}/repo/__aggr/reload?run=previous-process"))
            .send()
            .await
            .unwrap();
        let mut stream = response.bytes_stream();
        let event = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&event).contains("data: reload"));
        drop(stream);
        let response = client
            .get(format!("{origin}/repo/__aggr/reload?revision=99"))
            .send()
            .await
            .unwrap();
        let mut stream = response.bytes_stream();
        let event = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&event).contains("data: reload"));
        task.abort();
    }

    #[tokio::test]
    async fn shutdown_finishes_open_reload_streams_without_truncating_http() {
        let (origin, _, shutdown, task) = test_http_server().await;
        let response = reqwest::get(format!("{origin}/repo/__aggr/reload"))
            .await
            .unwrap();
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        shutdown.send_replace(true);
        let body = tokio::time::timeout(Duration::from_secs(2), response.bytes())
            .await
            .unwrap()
            .expect("a restarting worker must finish the HTTP body cleanly");
        assert!(body.is_empty());
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn completed_sources_are_visible_before_a_slow_source_finishes() {
        crate::http::install_crypto_provider();
        let tmp = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .arg(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        let origin = httpmock::MockServer::start_async().await;
        for (path, title, delay) in [
            ("/fast", "Fast article", 0),
            ("/slow", "Slow article", 5000),
        ] {
            origin.mock_async(|when, then| {
                when.path(path);
                then.status(200).delay(Duration::from_millis(delay)).body(format!(
                    "<rss version=\"2.0\"><channel><title>{title}</title><link>https://example.test/</link><item><title>{title}</title><link>https://example.test{path}</link><description>{title} has its own distinct body.</description></item></channel></rss>"
                ));
            }).await;
        }
        let config_path = tmp.path().join("aggr.toml");
        std::fs::write(&config_path, format!(
            "[defaults]\ndiscussions=[]\n[site]\npwa=false\n[[sources]]\nslug='fast'\nurl='{0}/fast'\n[[sources]]\nslug='slow'\nurl='{0}/slow'\n", origin.base_url()
        )).unwrap();
        let project = Arc::new(Project::load_offline(&config_path).await.unwrap());
        let (reload, _) = broadcast::channel(16);
        let state = DevState {
            data: tmp.path().join("data"),
            cache: tmp.path().join("cache"),
            cached: tmp.path().join("site"),
            staging: tmp.path().join("scratch/site"),
            site: MemorySite::loading(),
            reload,
            preparation: Arc::default(),
        };
        crate::store::Store::open(&state.data).bootstrap().unwrap();
        let site = state.site.clone();
        let syncing = tokio::spawn(async move {
            refresh(
                project,
                &FetchArgs::default(),
                &BuildArgs::default(),
                &state,
                true,
            )
            .await
        });
        let first = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let body = site.body("index.html").await.unwrap().unwrap();
                let html = String::from_utf8_lossy(&body);
                if html.contains("Fast article") {
                    assert!(!html.contains("Slow article"));
                    break;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        if first.is_err() {
            syncing.abort();
        }
        assert!(
            first.is_ok(),
            "a completed source must be served before the slow source finishes"
        );
        syncing.await.unwrap().unwrap();
        let body = site.body("index.html").await.unwrap().unwrap();
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains("Fast article") && html.contains("Slow article"));
    }

    #[tokio::test]
    async fn automatic_local_reprocessing_is_visible_before_remote_refresh() {
        crate::http::install_crypto_provider();
        let tmp = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .arg(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        let origin = httpmock::MockServer::start_async().await;
        origin.mock_async(|when, then| {
            when.path("/slow");
            then.status(200).delay(Duration::from_secs(5)).body("<rss version=\"2.0\"><channel><title>Feed</title><link>https://example.test/</link></channel></rss>");
        }).await;
        let config_path = tmp.path().join("aggr.toml");
        std::fs::write(&config_path, format!("[defaults]\ndiscussions=[]\n[site]\npwa=false\n[[sources]]\nslug='blog'\nurl='{}/slow'\n", origin.base_url())).unwrap();
        let project = Arc::new(Project::load_offline(&config_path).await.unwrap());
        let (reload, _) = broadcast::channel(16);
        let state = DevState {
            data: tmp.path().join("data"),
            cache: tmp.path().join("cache"),
            cached: tmp.path().join("site"),
            staging: tmp.path().join("scratch/site"),
            site: MemorySite::loading(),
            reload,
            preparation: Arc::default(),
        };
        let store = crate::store::Store::open(&state.data);
        store.bootstrap().unwrap();
        store.write_item(crate::store::NewItem {
            dir: "items/blog", stem: "retained",
            front: &crate::model::FrontMatter {
                source: "blog".into(), title: "Retained article".into(),
                link: "https://example.test/retained".into(), first_seen: Utc::now(),
                content: crate::model::ContentKind::Extracted,
                html: Some("retained.html".into()), ..Default::default()
            },
            body: "Stale local conversion.", html: Some("<p>Updated locally from the complete retained HTML companion, without waiting for the remote feed.</p>"),
            preview: None, images: &[],
        }).unwrap();
        let site = state.site.clone();
        let syncing = tokio::spawn(async move {
            refresh(
                project,
                &FetchArgs::default(),
                &BuildArgs::default(),
                &state,
                true,
            )
            .await
        });
        let first = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(body) = site.body("items/blog/retained/index.html").await.unwrap() {
                    let html = String::from_utf8_lossy(&body);
                    if html.contains("Updated locally from the complete retained HTML") {
                        assert!(!html.contains("Stale local conversion"));
                        break;
                    }
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        if first.is_err() {
            syncing.abort();
        }
        assert!(
            first.is_ok(),
            "local reprocessing must be published before waiting for remote sources"
        );
        syncing.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn readable_cached_snapshots_are_restored_and_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let cached = tmp.path().join("site");
        std::fs::create_dir_all(&cached).unwrap();
        std::fs::write(cached.join("index.html"), "restored").unwrap();
        assert!(
            MemorySite::cached(&cached).is_none(),
            "a directory without the ownership marker is not a snapshot"
        );
        std::fs::write(cached.join(".aggr-site"), "1").unwrap();
        let site = MemorySite::cached(&cached).unwrap();
        assert_eq!(
            site.response("/", "/").await.unwrap().1.as_slice(),
            b"restored"
        );
        assert!(cached.join("index.html").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_cached_snapshots_are_discarded_so_the_dev_key_stops_matching() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let cached = tmp.path().join("site");
        std::fs::create_dir_all(&cached).unwrap();
        std::fs::write(cached.join(".aggr-site"), "1").unwrap();
        std::fs::write(cached.join(DEV_KEY_FILE), "fingerprint").unwrap();
        std::fs::write(cached.join("index.html"), "stale").unwrap();
        std::fs::set_permissions(
            cached.join("index.html"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        if std::fs::read(cached.join("index.html")).is_ok() {
            // Running as root: an unreadable snapshot entry cannot be simulated.
            return;
        }
        assert!(dev_key_matches(&cached, "fingerprint"));
        assert!(MemorySite::cached(&cached).is_none());
        assert!(!cached.exists(), "the unusable snapshot is removed");
        assert!(!dev_key_matches(&cached, "fingerprint"));
    }

    #[test]
    fn resolves_under_the_root_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("items/a b")).unwrap();
        std::fs::write(root.join("index.html"), "").unwrap();
        std::fs::write(root.join("items/a b/index.html"), "").unwrap();
        std::fs::write(root.join("search.json"), "").unwrap();

        assert_eq!(resolve(root, "/", "/"), Some(root.join("index.html")));
        assert_eq!(
            resolve(root, "/", "/items/a%20b/"),
            Some(root.join("items/a b/index.html"))
        );
        assert_eq!(resolve(root, "/", "/items/a%20b"), None);
        assert_eq!(
            resolve(root, "/", "/search.json"),
            Some(root.join("search.json"))
        );
        assert_eq!(resolve(root, "/", "/nope/"), None);
        assert_eq!(resolve(root, "/", "/../Cargo.toml"), None);
        assert_eq!(resolve(root, "/", "/%2e%2e/Cargo.toml"), None);

        assert_eq!(
            resolve(root, "/repo/", "/repo/"),
            Some(root.join("index.html"))
        );
        assert_eq!(
            resolve(root, "/repo/", "/repo/search.json"),
            Some(root.join("search.json"))
        );
        assert_eq!(resolve(root, "/repo/", "/search.json"), None);
    }

    #[test]
    fn development_cache_keeps_hashed_assets_and_refuses_pages() {
        let sha1 = "0123456789abcdef0123456789abcdef01234567";
        let sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(cache_control("index.html"), PAGE_CACHE);
        assert_eq!(cache_control("browse/index.html"), PAGE_CACHE);
        assert!(!PAGE_CACHE.contains("Clear-Site-Data"));
        assert_eq!(
            cache_control(&format!("assets/images/{sha1}.webp")),
            ASSET_CACHE
        );
        assert_eq!(cache_control(&format!("assets/images/{sha1}")), ASSET_CACHE);
        assert_eq!(
            cache_control(&format!("assets/images/{}.webp", &sha1[1..])),
            FRESH_CACHE
        );
        assert_eq!(
            cache_control("assets/icons/icon-0123456789ab.svg"),
            ASSET_CACHE
        );
        assert_eq!(
            cache_control("assets/icons/icon-0123456789.svg"),
            FRESH_CACHE
        );
        assert_eq!(
            cache_control("assets/app/page.svelte-Ck4TY_xv.js"),
            ASSET_CACHE
        );
        assert_eq!(cache_control("assets/app/main.js"), FRESH_CACHE);
        assert_eq!(cache_control("assets/app/bootstrap.js"), FRESH_CACHE);
        assert_eq!(cache_control("assets/icons/icon-Ck4TY_xv.svg"), FRESH_CACHE);
        assert_eq!(
            cache_control(&format!("pagefind/{sha256}/x.pf_fragment")),
            ASSET_CACHE
        );
        assert_eq!(
            cache_control(&format!("pagefind/{}/x.pf_fragment", &sha256[1..])),
            FRESH_CACHE
        );
        assert_eq!(cache_control(&format!("items/{sha1}/")), FRESH_CACHE);
        assert_eq!(cache_control("feed.json"), FRESH_CACHE);
        assert_eq!(cache_control("error.txt"), FRESH_CACHE);
    }

    #[test]
    fn snapshots_reuse_only_immutable_bodies_and_remove_absent_files() {
        let tmp = tempfile::tempdir().unwrap();
        let write = |path: &str, body: &[u8]| {
            let path = tmp.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };
        let image = format!("assets/images/{}.webp", "a".repeat(40));
        let preview = format!("assets/previews/{}.webp", "b".repeat(40));
        let index = format!("pagefind/{}/pagefind.js", "c".repeat(64));
        let immutable = [&*image, &*preview, &*index, "assets/style-123456abcdef.css"];
        let mutable = [
            "index.html",
            "search-manifest.json",
            "offline-catalog.json",
            "sw.js",
            "pagefind/pagefind.js",
            "assets/custom.css",
        ];
        for path in immutable.iter().chain(mutable.iter()) {
            write(path, b"before");
        }
        write("removed.html", b"gone");
        let first = read_site(tmp.path(), None).unwrap();
        for path in mutable {
            write(path, b"after!");
        }
        std::fs::remove_file(tmp.path().join("removed.html")).unwrap();
        let changed = "assets/style-fedcba654321.css";
        write(changed, b"new asset");
        let second = read_site(tmp.path(), Some(&first)).unwrap();
        for path in immutable {
            assert!(Arc::ptr_eq(&first[path], &second[path]), "{path}");
        }
        for path in mutable {
            assert!(!Arc::ptr_eq(&first[path], &second[path]), "{path}");
            assert_eq!(second[path].as_slice(), b"after!");
        }
        assert!(!second.contains_key("removed.html"));
        assert_eq!(first["removed.html"].as_slice(), b"gone");
        assert_eq!(first["index.html"].as_slice(), b"before");
        assert_eq!(second[changed].as_slice(), b"new asset");
        write(&image, b"unexpected different length");
        let third = read_site(tmp.path(), Some(&second)).unwrap();
        assert!(!Arc::ptr_eq(&second[&image], &third[&image]));
        assert_eq!(third[&image].as_slice(), b"unexpected different length");
    }

    #[test]
    fn immutable_snapshot_paths_follow_generated_content_addresses() {
        for path in [
            "index.html",
            "search-manifest.json",
            "assets/app.js",
            "assets/style-notahash.css",
            "assets/images/photo.jpg",
            "pagefind/pagefind.js",
            "pagefind/latest/pagefind.js",
        ] {
            assert!(!immutable_site_file(path), "{path}");
        }
        assert!(immutable_site_file(
            "assets/fonts/reader-123456abcdef.woff2"
        ));
        assert!(immutable_site_file(&format!(
            "pagefind/{}/fragment/one.pf_fragment",
            "1".repeat(64)
        )));
    }

    #[tokio::test]
    async fn snapshots_and_atomically_replaces_transient_builds() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join("site");
        let cached = tmp.path().join("cached");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("index.html"), "first").unwrap();
        std::fs::write(staging.join("404.html"), "missing").unwrap();

        let site = MemorySite::load(&staging).await.unwrap();
        assert!(!staging.exists());
        let first_body = site.response("/", "/").await.unwrap().1;
        assert_eq!(first_body.as_slice(), b"first");
        assert!(Arc::ptr_eq(
            &first_body,
            &site.response("/", "/").await.unwrap().1
        ));

        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("index.html"), "second").unwrap();
        std::fs::write(staging.join("search.json"), "[]").unwrap();
        site.replace_from(&staging, &cached).await.unwrap();

        assert!(!staging.exists());
        assert!(cached.join("index.html").is_file());
        assert!(!cached.with_extension("previous").exists());
        assert_eq!(
            first_body.as_slice(),
            b"first",
            "in-flight responses retain their snapshot"
        );
        assert_eq!(
            site.response("/", "/").await.unwrap().1.as_slice(),
            b"second"
        );
        assert!(site.response("/", "/404.html").await.is_none());
        assert_eq!(
            site.response("/", "/search.json")
                .await
                .unwrap()
                .1
                .as_slice(),
            b"[]"
        );
    }

    #[tokio::test]
    async fn theme_refresh_preserves_resolved_remote_sources_without_probing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .arg(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        let server = httpmock::MockServer::start_async().await;
        let collection = server
            .mock_async(|when, then| {
                when.path("/subscriptions.opml");
                then.status(200).body(format!(
                    "<opml version=\"2.0\"><body><outline xmlUrl=\"{}/feed\"/></body></opml>",
                    server.base_url(),
                ));
            })
            .await;
        let feed = server.mock_async(|when, then| {
            when.path("/feed");
            then.status(200).body("<rss version=\"2.0\"><channel><title>Feed</title><link>https://example.com/</link></channel></rss>");
        }).await;
        let config_path = tmp.path().join("aggr.toml");
        std::fs::write(
            &config_path,
            format!(
                "[[sources]]\nurl = \"{}/subscriptions.opml\"\n",
                server.base_url(),
            ),
        )
        .unwrap();
        let project = Arc::new(Project::load(&config_path).await.unwrap());
        assert_eq!(project.sources.len(), 1);
        assert_eq!(project.sources[0].engine.url().unwrap().path(), "/feed");
        collection.assert_calls_async(1).await;
        feed.assert_calls_async(0).await;
        let retained = reload_project(project.clone(), false).await.unwrap();
        assert!(Arc::ptr_eq(&project, &retained));
        collection.assert_calls_async(1).await;
        feed.assert_calls_async(0).await;
    }

    #[tokio::test]
    async fn local_refresh_builds_retained_articles_without_fetching_sources() {
        use crate::model::{FrontMatter, file_stem, item_dir};
        use crate::store::{NewItem, Store};

        let tmp = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .arg(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        let server = httpmock::MockServer::start_async().await;
        let feed = server
            .mock_async(|when, then| {
                when.path("/feed");
                then.status(500);
            })
            .await;
        let config_path = tmp.path().join("aggr.toml");
        std::fs::write(&config_path, format!(
            "[defaults]\ndiscussions = [\"hackernews\"]\n[site]\npwa = false\n[[sources]]\nslug = \"blog\"\nurl = \"{}/feed\"\n",
            server.base_url(),
        )).unwrap();
        let project = Arc::new(Project::load_offline(&config_path).await.unwrap());
        let retained = reload_project(project.clone(), false).await.unwrap();
        assert!(Arc::ptr_eq(&project, &retained));
        let (reload, _) = broadcast::channel(16);
        let state = DevState {
            data: tmp.path().join("data"),
            cache: tmp.path().join("cache"),
            cached: tmp.path().join("site"),
            staging: tmp.path().join("staging"),
            site: MemorySite::loading(),
            reload,
            preparation: Arc::default(),
        };
        let store = Store::open(&state.data);
        store.bootstrap().unwrap();
        let date = Utc::now();
        let front = FrontMatter {
            title: "Retained article".into(),
            link: "https://example.test/article".into(),
            source: "blog".into(),
            first_seen: date,
            ..Default::default()
        };
        store
            .write_item(NewItem {
                dir: &item_dir("blog", date),
                stem: &file_stem(date, &front.title),
                front: &front,
                body: "Already captured before the previous run stopped.",
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let fetch = FetchArgs::default();
        let build = BuildArgs::default();
        assert!(
            refresh(project.clone(), &fetch, &build, &state, false)
                .await
                .unwrap()
        );
        let root = state.site.lazy_root().await.unwrap();
        let retained_snapshot = match &*state.site.snapshot.read().await {
            Snapshot::Lazy(site) => Arc::clone(site),
            Snapshot::Files(_) | Snapshot::Pending(_) => panic!("ordinary dev must be lazy"),
        };
        assert!(
            !root.join("index.html").exists(),
            "preparation does not render the feed"
        );
        assert!(
            !root.join("items").exists(),
            "unrequested articles stay unrendered"
        );
        assert!(
            !root.join("pagefind").exists(),
            "search is initialized on demand"
        );
        let page = state.site.response("/", "/").await.unwrap().1;
        assert!(
            std::str::from_utf8(&page)
                .unwrap()
                .contains("Retained article")
        );
        assert!(root.join("index.html").is_file());
        assert!(
            !root.join("items").exists(),
            "the feed does not render article pages"
        );
        assert!(
            !crate::cache::Namespace::Discussions
                .dir(&state.cache)
                .exists(),
            "local rendering must not initiate discussion resolution"
        );
        assert!(
            !refresh(project, &fetch, &build, &state, false)
                .await
                .unwrap()
        );
        assert_eq!(
            state.site.lazy_root().await.as_deref(),
            Some(root.as_path())
        );
        feed.assert_calls_async(0).await;

        let project = Arc::new(Project::load_offline(&config_path).await.unwrap());
        let front = FrontMatter {
            title: "Newly synced article".into(),
            link: "https://example.test/fresh".into(),
            ..front
        };
        let stem = file_stem(date, &front.title);
        store
            .write_item(NewItem {
                dir: &item_dir("blog", date),
                stem: &stem,
                front: &front,
                body: "Freshly captured content",
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let mut reloads = state.reload.subscribe();
        let fresh = async {
            let page = state.site.response("/", "/").await.unwrap().1;
            assert!(
                std::str::from_utf8(&page)
                    .unwrap()
                    .contains("Newly synced article"),
                "new articles must be visible before discussion lookups start"
            );
            assert!(reloads.try_recv().is_ok());
            Ok(crate::discussions::ResolutionSet::default())
        };
        assert!(
            render_with_discussion_refresh(
                project.clone(),
                &build,
                &state,
                crate::discussions::ResolutionSet::default(),
                true,
                fresh
            )
            .await
            .unwrap()
        );
        assert!(
            reloads.try_recv().is_err(),
            "unchanged discussions must not rebuild or reload again"
        );
        assert_ne!(
            state.site.lazy_root().await.as_deref(),
            Some(root.as_path())
        );
        assert!(root.exists(), "an in-flight snapshot keeps its directory");
        assert!(
            std::str::from_utf8(&retained_snapshot.response("index.html").unwrap().unwrap())
                .unwrap()
                .contains("Retained article")
        );
        drop(retained_snapshot);
        assert!(
            !root.exists(),
            "superseded snapshots release their scratch directory"
        );
        let enriched: crate::discussions::ResolutionSet =
            serde_json::from_value(serde_json::json!({
                "hackernews:https://example.test/fresh": {
                    "url": "https://news.ycombinator.com/item?id=42", "score": 42
                }
            }))
            .unwrap();
        assert!(
            render_with_discussion_refresh(
                project.clone(),
                &build,
                &state,
                crate::discussions::ResolutionSet::default(),
                false,
                async { Ok(enriched.clone()) }
            )
            .await
            .unwrap()
        );
        assert!(reloads.try_recv().is_ok());
        assert!(reloads.try_recv().is_err());
        let article = state
            .site
            .response("/", &format!("/items/blog/{stem}/"))
            .await
            .unwrap()
            .1;
        assert!(
            std::str::from_utf8(&article)
                .unwrap()
                .contains("https://news.ycombinator.com/item?id=42")
        );
        assert!(
            !render_with_discussion_refresh(
                project.clone(),
                &build,
                &state,
                enriched,
                false,
                async { anyhow::bail!("provider unavailable") }
            )
            .await
            .unwrap()
        );
        assert!(
            reloads.try_recv().is_err(),
            "provider failure keeps the current readable snapshot"
        );
        let removed = store
            .items()
            .unwrap()
            .into_iter()
            .find(|item| item.front.link == "https://example.test/fresh")
            .unwrap();
        store.remove_item(&removed.path).unwrap();
        assert!(
            refresh(project, &fetch, &build, &state, false)
                .await
                .unwrap()
        );
        assert!(
            state
                .site
                .response("/", &format!("/items/blog/{stem}/"))
                .await
                .is_none(),
            "a route absent from the next snapshot must not serve its old generated file"
        );

        let last_good = state.site.lazy_root().await.unwrap();
        // A config edit forces a new generation; an unreadable source status file makes its
        // preparation fail, and the last good generation keeps serving.
        let config = std::fs::read_to_string(&config_path)
            .unwrap()
            .replace("pwa = false", "pwa = false\ntitle = \"Renamed\"");
        std::fs::write(&config_path, config).unwrap();
        let unreadable_status = state.data.join("status.toml");
        std::fs::create_dir(&unreadable_status).unwrap();
        let project = Arc::new(Project::load_offline(&config_path).await.unwrap());
        assert!(
            refresh(project.clone(), &fetch, &build, &state, false)
                .await
                .is_err()
        );
        assert_eq!(
            state.site.lazy_root().await.as_deref(),
            Some(last_good.as_path())
        );
        assert!(
            std::str::from_utf8(&state.site.response("/", "/").await.unwrap().1)
                .unwrap()
                .contains("Retained article")
        );
        std::fs::remove_dir(&unreadable_status).unwrap();
        assert!(
            refresh(project, &fetch, &build, &state, false)
                .await
                .unwrap()
        );
        assert_ne!(
            state.site.lazy_root().await.as_deref(),
            Some(last_good.as_path())
        );
        feed.assert_calls_async(0).await;
    }

    #[test]
    fn content_types() {
        assert_eq!(
            content_type(Path::new("a/index.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(content_type(Path::new("x.json")), "application/json");
        assert_eq!(
            content_type(Path::new("items/a/post.image-0123456789ab.avif")),
            "image/avif"
        );
        assert_eq!(
            content_type(Path::new("assets/documents/paper.pdf")),
            "application/pdf"
        );
        assert_eq!(
            content_type(Path::new("items/source/post.json")),
            "application/ld+json; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("sources/a/feed.json")),
            "application/feed+json; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("atom.xml")),
            "application/atom+xml; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("rss.xml")),
            "application/rss+xml; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("opensearch.xml")),
            "application/opensearchdescription+xml; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("article.rst")),
            "text/x-rst; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("manifest.webmanifest")),
            "application/manifest+json"
        );
        assert_eq!(content_type(Path::new("CNAME")), "application/octet-stream");
    }

    #[tokio::test]
    async fn busy_dev_port_falls_back_to_an_available_one() {
        let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = occupied.local_addr().unwrap().port();
        let listener = bind(port).await.unwrap();
        assert_ne!(listener.local_addr().unwrap().port(), port);
    }

    #[test]
    fn injects_reload_client_without_touching_the_build() {
        let html = inject_reload(b"<body>Hello</body>", "/repo/", "run-1", 3);
        let html = String::from_utf8(html).unwrap();
        assert!(html.contains("new EventSource('/repo/__aggr/reload?run=run-1&revision=3')"));
        assert!(html.contains("caches.delete"));
        assert!(html.contains("__aggr_dev"));
        assert!(!html.contains("window.AGGR"));
        assert!(!html.contains("aggr:build"));
        assert!(html.contains("n.searchParams.set('__aggr_dev',Date.now());location.replace(n)"));
        assert!(html.ends_with("</body>"));
    }

    #[test]
    fn ignores_generated_and_internal_changes() {
        let root = Path::new("/project");
        let configs = vec![root.join("aggr.toml")];
        let themes = vec![root.join("src/theme")];
        let excluded = vec![root.join("src/theme/generated")];
        assert!(watched(
            &root.join("aggr.toml"),
            &configs,
            &themes,
            &excluded
        ));
        assert!(watched(
            &root.join("src/theme/templates/base.html"),
            &configs,
            &themes,
            &excluded
        ));
        assert!(!watched(
            &root.join("readme.md"),
            &configs,
            &themes,
            &excluded
        ));
        assert!(!watched(
            &root.join("src/theme/generated/index.html"),
            &configs,
            &themes,
            &excluded
        ));
        assert!(!watched(
            &root.join(".aggr/data/item.md"),
            &configs,
            &themes,
            &excluded
        ));
        assert!(!watched(
            &root.join("target/debug/aggr"),
            &configs,
            &themes,
            &excluded
        ));
        assert!(!rebuild_event(notify::EventKind::Access(
            notify::event::AccessKind::Read
        )));
        assert!(!rebuild_event(notify::EventKind::Modify(
            notify::event::ModifyKind::Metadata(notify::event::MetadataKind::AccessTime)
        )));
        assert!(rebuild_event(notify::EventKind::Modify(
            notify::event::ModifyKind::Any
        )));
        assert!(watched(
            &root.join("src/theme"),
            &configs,
            &themes,
            &excluded
        ));
    }

    #[test]
    fn identical_dependency_events_do_not_rebuild_a_current_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(DEV_KEY_FILE), "same").unwrap();
        std::fs::write(temp.path().join(".aggr-site"), "1").unwrap();

        assert!(!rebuild_required(false, temp.path(), "same"));
        assert!(rebuild_required(false, temp.path(), "changed"));
        assert!(rebuild_required(true, temp.path(), "same"));
    }

    #[tokio::test]
    async fn dropping_the_watcher_cancels_queued_reload_work() {
        struct MarksDrop(Arc<AtomicBool>);
        impl Drop for MarksDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));
        let marker = MarksDrop(dropped.clone());
        let task = tokio::spawn(async move {
            let _marker = marker;
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        let guard = DevWatcher { task };

        drop(guard);
        tokio::time::timeout(Duration::from_secs(1), async {
            while !dropped.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
