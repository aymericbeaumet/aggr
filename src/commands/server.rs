//! The private live-reload HTTP server used by `aggr dev`; not intended for production traffic.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use chrono::Utc;
use notify::{RecursiveMode, Watcher as _};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{RwLock, broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};

use super::{Project, build, sync};
use crate::cli::{BuildArgs, DevArgs, FetchArgs};
use crate::git::Worktree;

const DEV_KEY_FILE: &str = ".aggr-dev-key";

mod client;

type SiteFiles = BTreeMap<String, Arc<Vec<u8>>>;

#[derive(Clone)]
struct MemorySite {
    files: Arc<RwLock<SiteFiles>>,
}

impl MemorySite {
    fn loading() -> Self {
        let page = b"<!doctype html><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>aggr dev</title><style>body{font:16px system-ui;margin:3rem;max-width:40rem}body:before{content:'';display:inline-block;width:.75rem;height:.75rem;margin-right:.6rem;border:2px solid #8ea1ff;border-top-color:transparent;border-radius:50%;animation:s 1s linear infinite}@keyframes s{to{transform:rotate(360deg)}}</style><p>Syncing sources and building the in-memory site&hellip;</p>".to_vec();
        let page = Arc::new(page);
        Self {
            files: Arc::new(RwLock::new(BTreeMap::from([
                ("index.html".to_string(), page.clone()),
                ("404.html".to_string(), page),
            ]))),
        }
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
                files: Arc::new(RwLock::new(files)),
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
            files: Arc::new(RwLock::new(files)),
        })
    }

    async fn replace_from(&self, staging: &Path, cached: &Path) -> Result<()> {
        let retained = self.files.read().await.clone();
        let staging = staging.to_owned();
        let cached = cached.to_owned();
        let files = tokio::task::spawn_blocking(move || -> Result<_> {
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
        *self.files.write().await = files;
        Ok(())
    }

    async fn response(&self, base: &str, path: &str) -> Option<(String, Arc<Vec<u8>>)> {
        let key = resolve_key(base, path)?;
        self.files
            .read()
            .await
            .get(&key)
            .cloned()
            .map(|body| (key, body))
    }

    async fn not_found(&self) -> (String, Arc<Vec<u8>>) {
        let key = "404.html".to_string();
        let body = self
            .files
            .read()
            .await
            .get(&key)
            .cloned()
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

pub async fn run_with_reload(
    project: Project,
    args: &DevArgs,
    data: PathBuf,
    cache: PathBuf,
    cached: PathBuf,
    staging: PathBuf,
) -> Result<()> {
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
    let site = match MemorySite::cached(&cached) {
        Some(site) => {
            println!("restored the previous dev build from cache");
            site
        }
        None => MemorySite::loading(),
    };
    let (reload, _) = broadcast::channel(16);
    let serving = host(listener, site.clone(), &base, reload.clone());
    let state = DevState {
        data,
        cache,
        cached,
        staging,
        site,
        reload,
    };
    // Register filesystem watches before the first network sync/build. Editors can otherwise save
    // a config or theme while startup is busy and leave the served snapshot stale until the next
    // manual edit.
    let pending_watcher = prepare_watch(&project, &state)?;
    let initializing = async {
        match refresh(project.clone(), &args.fetch, &build_args, &state, false).await {
            Ok(_) => println!("cached articles ready: {url}"),
            Err(err) => eprintln!("cached build failed: {err:#}"),
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
                project.clone()
            }
        };
        let _watcher = pending_watcher.start(project, args, state)?;
        std::future::pending::<Result<()>>().await
    };
    let result = tokio::select! {
        result = serving => result,
        result = initializing => result,
        signal = &mut shutdown_rx => {
            signal.context("Ctrl-C task stopped")?.context("listening for Ctrl-C")?;
            println!("stopping");
            Ok(())
        }
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
    let worktree = Worktree::ephemeral(state.data.clone());
    let visible_change = if sync_sources {
        let report = sync::run_dev(&project, &worktree, fetch_args, &state.cache).await?;
        println!(
            "dev data: {} new item(s) in the isolated system cache (never committed or pushed)",
            report.added()
        );
        report.added() > 0
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
    let discussions =
        crate::discussions::cached(&project.config.networks, &store.items()?, &state.cache);
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
    if !project
        .config
        .networks
        .iter()
        .any(|network| network.provider.is_some())
    {
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
    let base_url = build::base_url(&project, build_args)?;
    let now = Utc::now();
    let build_args = build_args.clone();
    let data = state.data.clone();
    let cache = state.cache.clone();
    let cached = state.cached.clone();
    let staging = state.staging.clone();
    let (rendered, finished) = oneshot::channel();
    // A dedicated thread keeps template rendering from blocking the signal future. Unlike
    // `spawn_blocking`, it also cannot make Tokio wait for a long render while shutting down;
    // interrupted output stays in the disposable staging directory and is cleared next run.
    std::thread::Builder::new()
        .name("aggr-dev-render".to_string())
        .spawn(move || {
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
            );
            let _ = rendered.send(result);
        })
        .context("starting dev render worker")?;
    let rebuilt = finished
        .await
        .context("dev render worker stopped before finishing")??;
    if rebuilt {
        state
            .site
            .replace_from(&state.staging, &state.cached)
            .await?;
        let _ = state.reload.send(());
    }
    Ok(rebuilt)
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
) -> Result<bool> {
    let store = crate::store::Store::open(data);
    let generation = crate::site::render_generation(&store.items()?, &project.config.site, now);
    let discussions_fingerprint = discussions.fingerprint();
    let config_sha = project.config_sha();
    let fingerprint = crate::cache::render_fingerprint(crate::cache::RenderFingerprint {
        config: &project.config,
        project_root: &project.root,
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
        return Ok(false);
    }
    // The staging directory is disposable. A template error may leave a partial tree without the
    // safety marker used for user-selected output directories, so always reset it before retrying.
    remove_build(staging)?;
    build::run_ephemeral(project, build_args, data, staging, cache, discussions)?;
    crate::cache::write(&staging.join(DEV_KEY_FILE), fingerprint.as_bytes())
        .context("writing the dev build fingerprint")?;
    Ok(true)
}

fn rebuild_required(visible_change: bool, site: &Path, fingerprint: &str) -> bool {
    visible_change || !dev_key_matches(site, fingerprint)
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
) -> Result<()> {
    let client = client::ClientDev::from_env()?.map(Arc::new);
    let base = base.to_string();
    let run = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string();
    loop {
        let (stream, _) = listener.accept().await?;
        let site = site.clone();
        let base = base.clone();
        let run = run.clone();
        let reload = reload.clone();
        let client = client.clone();
        tokio::spawn(async move {
            if let Err(err) = handle(stream, &site, &base, &run, &reload, client.as_deref()).await {
                log::debug!("dev server: {err:#}");
            }
        });
    }
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

        // Keep absent override directories as logical dependencies. Their nearest existing parent
        // is watched shallowly so creating `templates/`, `static/`, or a configured theme starts
        // recursive watching without restarting dev.
        let mut themes = vec![project.root.join("templates"), project.root.join("static")];
        if project.config.site.theme == "default" {
            #[cfg(debug_assertions)]
            {
                let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("themes/default");
                if source.is_dir() && !source.starts_with(&project.root) {
                    themes.push(source);
                }
            }
        } else {
            themes.push(project.root.join(&project.config.site.theme));
        }
        themes = themes
            .into_iter()
            .map(normalize_watch_path)
            .collect::<Result<Vec<_>>>()?;
        themes.sort();
        themes.dedup();

        let mut excluded = vec![
            state.data.clone(),
            state.cache.clone(),
            state.cached.clone(),
            state.cached.with_extension("previous"),
            state.staging.clone(),
        ];
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
        for theme in themes.iter().filter(|path| !path.is_dir()) {
            if let Some(parent) = nearest_existing_parent(theme)
                && parent.starts_with(&project.root)
            {
                shallow.push(parent);
            }
        }
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

fn nearest_existing_parent(path: &Path) -> Option<PathBuf> {
    let mut parent = path.parent()?;
    loop {
        if parent.is_dir() {
            return Some(parent.to_path_buf());
        }
        parent = parent.parent()?;
    }
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

async fn handle(
    stream: TcpStream,
    site: &MemorySite,
    base: &str,
    run: &str,
    reload: &broadcast::Sender<()>,
    client: Option<&client::ClientDev>,
) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;
    // Drain the headers; we never need them.
    let mut line = String::new();
    while reader.read_line(&mut line).await? > 0 && line != "\r\n" && line != "\n" {
        line.clear();
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or("/");
    let path = target.split(['?', '#']).next().unwrap_or("/");
    if path == format!("{base}__aggr/reload") {
        let stream = reader.get_mut();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: keep-alive\r\n\r\n")
            .await?;
        let mut updates = reload.subscribe();
        while updates.recv().await.is_ok() {
            stream.write_all(b"data: reload\n\n").await?;
            stream.flush().await?;
        }
        return Ok(());
    }
    let (status, file, mut body) = match site.response(base, path).await {
        Some((file, body)) => ("200 OK", file, body),
        None => {
            let (file, body) = site.not_found().await;
            ("404 Not Found", file, body)
        }
    };
    let file = Path::new(&file);
    if file
        .extension()
        .is_some_and(|extension| extension == "html")
    {
        body = Arc::new(inject_reload(&body, base, run));
        if let Some(client) = client {
            body = Arc::new(client.inject(&body));
        }
    }
    let cache_headers = if file
        .extension()
        .is_some_and(|extension| extension == "html")
    {
        "Cache-Control: no-store, no-cache, must-revalidate, max-age=0\r\nPragma: no-cache\r\nExpires: 0\r\nClear-Site-Data: \"cache\"\r\n"
    } else {
        "Cache-Control: no-store\r\n"
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{cache_headers}Connection: close\r\n\r\n",
        content_type(file),
        body.len()
    );
    let stream = reader.get_mut();
    stream.write_all(head.as_bytes()).await?;
    if method != "HEAD" {
        stream.write_all(&body).await?;
    }
    stream.shutdown().await?;
    Ok(())
}

fn inject_reload(body: &[u8], base: &str, run: &str) -> Vec<u8> {
    let script = format!(
        "<script>(function(){{if(window.AGGR)window.AGGR.pwa=false;var b=new URL({0:?},location.origin),u=new URL(location.href);if(u.searchParams.delete('__aggr_dev'))history.replaceState(null,'',u);if('serviceWorker'in navigator)navigator.serviceWorker.getRegistration(b).then(function(r){{if(r)r.unregister()}});if('caches'in window){{var p='aggr:'+encodeURIComponent(b.pathname)+':';caches.keys().then(function(k){{k.filter(function(x){{return x.indexOf(p)===0}}).forEach(function(x){{caches.delete(x)}})}})}}new EventSource('{0}__aggr/reload?run={1}').onmessage=function(){{if(!window.dispatchEvent(new Event('aggr:build',{{cancelable:true}})))return;var n=new URL(location.href);n.searchParams.set('__aggr_dev',Date.now());location.replace(n)}}}})();</script>",
        base, run
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
            "[site]\npwa = false\n[[sources]]\nslug = \"blog\"\nurl = \"{}/feed\"\n[[networks]]\nprovider = \"hackernews\"\n",
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
        let page = state.site.response("/", "/").await.unwrap().1;
        assert!(
            std::str::from_utf8(&page)
                .unwrap()
                .contains("Retained article")
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
            !render_with_discussion_refresh(project, &build, &state, enriched, false, async {
                anyhow::bail!("provider unavailable")
            })
            .await
            .unwrap()
        );
        assert!(
            reloads.try_recv().is_err(),
            "provider failure keeps the current readable snapshot"
        );
    }

    #[test]
    fn content_types() {
        assert_eq!(
            content_type(Path::new("a/index.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(content_type(Path::new("x.json")), "application/json");
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
        let html = inject_reload(b"<body>Hello</body>", "/repo/", "run-1");
        let html = String::from_utf8(html).unwrap();
        assert!(html.contains("new EventSource('/repo/__aggr/reload?run=run-1')"));
        assert!(html.contains("caches.delete"));
        assert!(html.contains("window.AGGR.pwa=false"));
        assert!(html.contains("__aggr_dev"));
        assert!(html.contains(
            "if(!window.dispatchEvent(new Event('aggr:build',{cancelable:true})))return;"
        ));
        assert!(html.contains("n.searchParams.set('__aggr_dev',Date.now());location.replace(n)"));
        assert!(html.ends_with("</body>"));
    }

    #[test]
    fn ignores_generated_and_internal_changes() {
        let root = Path::new("/project");
        let configs = vec![root.join("aggr.toml")];
        let themes = vec![root.join("templates")];
        let excluded = vec![root.join("templates/generated")];
        assert!(watched(
            &root.join("aggr.toml"),
            &configs,
            &themes,
            &excluded
        ));
        assert!(watched(
            &root.join("templates/base.html"),
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
            &root.join("templates/generated/index.html"),
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
            &root.join("templates"),
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
