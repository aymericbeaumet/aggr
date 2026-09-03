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
use tokio::time::{Duration, sleep};

use super::{Project, build, sync};
use crate::cli::{BuildArgs, DevArgs, FetchArgs};
use crate::git::Worktree;

const DEV_KEY_FILE: &str = ".aggr-dev-key";

#[derive(Clone)]
struct MemorySite {
    files: Arc<RwLock<BTreeMap<String, Vec<u8>>>>,
}

impl MemorySite {
    fn loading() -> Self {
        let page = b"<!doctype html><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>aggr dev</title><style>body{font:16px system-ui;margin:3rem;max-width:40rem}body:before{content:'';display:inline-block;width:.75rem;height:.75rem;margin-right:.6rem;border:2px solid #8ea1ff;border-top-color:transparent;border-radius:50%;animation:s 1s linear infinite}@keyframes s{to{transform:rotate(360deg)}}</style><p>Syncing sources and building the in-memory site&hellip;</p>".to_vec();
        Self {
            files: Arc::new(RwLock::new(BTreeMap::from([
                ("index.html".to_string(), page.clone()),
                ("404.html".to_string(), page),
            ]))),
        }
    }

    fn cached(root: &Path) -> Option<Self> {
        root.join(".aggr-site")
            .is_file()
            .then(|| read_site(root))
            .transpose()
            .ok()
            .flatten()
            .map(|files| Self {
                files: Arc::new(RwLock::new(files)),
            })
    }

    #[cfg(test)]
    async fn load(staging: &Path) -> Result<Self> {
        let files = read_site(staging)?;
        remove_build(staging)?;
        Ok(Self {
            files: Arc::new(RwLock::new(files)),
        })
    }

    async fn replace_from(&self, staging: &Path, cached: &Path) -> Result<()> {
        let files = read_site(staging)?;
        if cached.exists() {
            remove_build(cached)?;
        }
        if let Some(parent) = cached.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::rename(staging, cached).with_context(|| {
            format!(
                "promoting dev build {} to {}",
                staging.display(),
                cached.display()
            )
        })?;
        *self.files.write().await = files;
        Ok(())
    }

    async fn response(&self, base: &str, path: &str) -> Option<(String, Vec<u8>)> {
        let key = resolve_key(base, path)?;
        self.files
            .read()
            .await
            .get(&key)
            .cloned()
            .map(|body| (key, body))
    }

    async fn not_found(&self) -> (String, Vec<u8>) {
        let key = "404.html".to_string();
        let body = self
            .files
            .read()
            .await
            .get(&key)
            .cloned()
            .unwrap_or_else(|| b"not found".to_vec());
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

fn read_site(root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
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
        files.insert(
            key,
            std::fs::read(entry.path())
                .with_context(|| format!("reading {}", entry.path().display()))?,
        );
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
    project: &Project,
    args: &DevArgs,
    data: PathBuf,
    cache: PathBuf,
    cached: PathBuf,
    staging: PathBuf,
) -> Result<()> {
    // Install the process signal handler before the listener becomes visible. This makes even an
    // immediate Ctrl-C (common when a command was started by mistake) a graceful shutdown.
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let shutdown_task = tokio::spawn(async move {
        let _ = shutdown_tx.send(tokio::signal::ctrl_c().await);
    });
    tokio::task::yield_now().await;
    let base = crate::site::base_path(build::base_url(project, &args.build)?.as_deref());
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
    let initializing = async {
        match refresh(project, &args.fetch, &args.build, &state, true).await {
            Ok(rebuilt) => {
                println!("ready: {url}");
                if rebuilt {
                    let _ = state.reload.send(());
                }
            }
            Err(err) => eprintln!("initial build failed: {err:#}"),
        }
        let _watcher = watch(project, args, state)?;
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
    project: &Project,
    fetch_args: &FetchArgs,
    build_args: &BuildArgs,
    state: &DevState,
    sync_sources: bool,
) -> Result<bool> {
    let base_url = build::base_url(project, build_args)?;
    let worktree = Worktree::ephemeral(state.data.clone());
    let visible_change = if sync_sources {
        let report = sync::run_dev(project, &worktree, fetch_args, &state.cache).await?;
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
        true
    };
    let store = crate::store::Store::open(&state.data);
    let discussions = build::resolve_discussions(project, &store, &state.cache, Utc::now()).await?;
    let discussions_fingerprint = discussions.fingerprint();
    let fingerprint = crate::cache::render_fingerprint(
        &project.config,
        &project.root,
        project.config_sha().as_deref(),
        None,
        base_url.as_deref(),
        build_args.release,
        Some(&discussions_fingerprint),
    )?;
    if !visible_change && dev_key_matches(&state.cached, &fingerprint) {
        println!("dev build already current");
        return Ok(false);
    }
    build::run_ephemeral(
        project,
        build_args,
        &state.data,
        &state.staging,
        discussions,
    )?;
    std::fs::write(state.staging.join(DEV_KEY_FILE), fingerprint)
        .context("writing the dev build fingerprint")?;
    state
        .site
        .replace_from(&state.staging, &state.cached)
        .await
        .map(|()| true)
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
        tokio::spawn(async move {
            if let Err(err) = handle(stream, &site, &base, &run, &reload).await {
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

fn watch(project: &Project, args: &DevArgs, state: DevState) -> Result<notify::RecommendedWatcher> {
    let (events, mut changes) = mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = events.send(event);
    })?;
    let mut roots = vec![project.root.clone()];
    roots.extend(crate::site::theme_layers(&project.config, &project.root)?.dirs);
    roots.sort();
    roots.dedup();
    let mut watched_roots: Vec<PathBuf> = Vec::new();
    for root in roots {
        if !watched_roots.iter().any(|parent| root.starts_with(parent)) {
            watched_roots.push(root);
        }
    }
    for root in &watched_roots {
        watcher
            .watch(root, RecursiveMode::Recursive)
            .with_context(|| format!("watching {}", root.display()))?;
        log::info!("watching {}", root.display());
    }

    let config_path = project.config_path.clone();
    let build_args = crate::cli::BuildArgs {
        out: args.build.out.clone(),
        base_url: args.build.base_url.clone(),
        data_ref: args.build.data_ref.clone(),
        release: args.build.release,
    };
    let fetch_args = args.fetch.clone();
    tokio::spawn(async move {
        while let Some(event) = changes.recv().await {
            let Ok(event) = event else { continue };
            if !event.paths.iter().any(|path| watched(path, &state.staging)) {
                continue;
            }
            let mut changed = event.paths;
            sleep(Duration::from_millis(150)).await;
            while let Ok(event) = changes.try_recv() {
                if let Ok(event) = event {
                    changed.extend(event.paths);
                }
            }
            let sync_sources = changed.iter().any(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
            });
            let result = match Project::load(&config_path) {
                Ok(project) => {
                    refresh(&project, &fetch_args, &build_args, &state, sync_sources).await
                }
                Err(err) => Err(err),
            };
            match result {
                Ok(true) => {
                    println!("reloaded");
                    let _ = state.reload.send(());
                }
                Ok(false) => println!("already current"),
                Err(err) => eprintln!("build failed: {err:#}"),
            }
        }
    });
    Ok(watcher)
}

fn watched(path: &Path, out: &Path) -> bool {
    !path.starts_with(out)
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
        body = inject_reload(&body, base, run);
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
        "<script>(function(){{if(window.AGGR)window.AGGR.pwa=false;var u=new URL(location.href);if(u.searchParams.delete('__aggr_dev'))history.replaceState(null,'',u);if('serviceWorker'in navigator)navigator.serviceWorker.getRegistrations().then(function(r){{r.forEach(function(x){{x.unregister()}})}});if('caches'in window)caches.keys().then(function(k){{k.forEach(function(x){{caches.delete(x)}})}});new EventSource('{}__aggr/reload?run={}').onmessage=function(){{var n=new URL(location.href);n.searchParams.set('__aggr_dev',Date.now());location.replace(n)}}}})();</script>",
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
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
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
        assert_eq!(site.response("/", "/").await.unwrap().1, b"first");

        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("index.html"), "second").unwrap();
        std::fs::write(staging.join("search.json"), "[]").unwrap();
        site.replace_from(&staging, &cached).await.unwrap();

        assert!(!staging.exists());
        assert!(cached.join("index.html").is_file());
        assert_eq!(site.response("/", "/").await.unwrap().1, b"second");
        assert!(site.response("/", "/404.html").await.is_none());
        assert_eq!(site.response("/", "/search.json").await.unwrap().1, b"[]");
    }

    #[test]
    fn content_types() {
        assert_eq!(
            content_type(Path::new("a/index.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(content_type(Path::new("x.json")), "application/json");
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
        assert!(html.ends_with("</body>"));
    }

    #[test]
    fn ignores_generated_and_internal_changes() {
        let root = Path::new("/project");
        let out = root.join("_site");
        assert!(watched(&root.join("aggr.toml"), &out));
        assert!(watched(&root.join("templates/base.html"), &out));
        assert!(!watched(&out.join("index.html"), &out));
        assert!(!watched(&root.join(".aggr/data/item.md"), &out));
        assert!(!watched(&root.join("target/debug/aggr"), &out));
    }
}
