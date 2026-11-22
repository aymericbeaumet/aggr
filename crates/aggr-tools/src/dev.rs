//! Contributor development: native UI compilation and a stable, byte-opaque preview origin.

use anyhow::{Context as _, Result, ensure};
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse as _, Response},
};
use notify::{RecursiveMode, Watcher as _};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    process::{Child, Command},
    sync::{RwLock, mpsc, oneshot},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Context {
    repository: PathBuf,
    cwd: PathBuf,
    executable: PathBuf,
    worker_args: Vec<String>,
    public_origin: String,
    strict_port: bool,
}

#[derive(Clone)]
struct Relay {
    public_origin: String,
    backend: Arc<RwLock<Option<String>>>,
    client: reqwest::Client,
    cache: Arc<Mutex<Responses>>,
}

#[derive(Clone)]
struct Cached {
    status: StatusCode,
    headers: HeaderMap,
    body: bytes::Bytes,
}

#[derive(Default)]
struct Responses {
    entries: BTreeMap<String, Cached>,
    order: VecDeque<String>,
    bytes: usize,
}

impl Responses {
    fn insert(&mut self, key: String, value: Cached) {
        const MAX_BYTES: usize = 32 * 1024 * 1024;
        if value.body.len() > 4 * 1024 * 1024 {
            return;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.body.len();
        }
        self.order.retain(|old| old != &key);
        self.bytes += value.body.len();
        self.order.push_back(key.clone());
        self.entries.insert(key, value);
        while self.bytes > MAX_BYTES || self.entries.len() > 128 {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some(value) = self.entries.remove(&old) {
                self.bytes -= value.body.len();
            }
        }
    }
}

fn cached_response(cached: Cached) -> Response {
    let mut response = Response::new(Body::from(cached.body));
    *response.status_mut() = cached.status;
    *response.headers_mut() = cached.headers;
    response
}

fn rewrite_location(headers: &mut HeaderMap, backend: &str, public: &str) {
    let Some(value) = headers
        .get("location")
        .and_then(|value| value.to_str().ok())
    else {
        return;
    };
    if !value.starts_with("http://") && !value.starts_with("https://") && !value.starts_with("//") {
        return;
    }
    let Ok(base) = reqwest::Url::parse(backend) else {
        return;
    };
    let Ok(location) = base.join(value) else {
        return;
    };
    if location.origin() != base.origin() {
        return;
    }
    let origin = location.origin().ascii_serialization();
    let Some(suffix) = location.as_str().strip_prefix(&origin) else {
        return;
    };
    if let Ok(value) = format!("{public}{suffix}").parse() {
        headers.insert("location", value);
    }
}

async fn relay(State(state): State<Relay>, request: Request) -> Response {
    let key = request
        .uri()
        .path_and_query()
        .map_or("/", |value| value.as_str())
        .to_owned();
    let method = request.method().clone();
    let fallback = || {
        if method != axum::http::Method::GET && method != axum::http::Method::HEAD {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        state
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .get(&key)
            .cloned()
            .map(|cached| {
                let mut response = cached_response(cached);
                if method == axum::http::Method::HEAD {
                    *response.body_mut() = Body::empty();
                }
                response
            })
            .unwrap_or_else(|| {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [("retry-after", "1"), ("cache-control", "no-store")],
                    "The development worker is restarting.",
                )
                    .into_response()
            })
    };
    let Some(origin) = state.backend.read().await.clone() else {
        return fallback();
    };
    let (parts, body) = request.into_parts();
    let is_get = parts.method == axum::http::Method::GET;
    let body = match axum::body::to_bytes(body, 8 * 1024 * 1024).await {
        Ok(body) => body,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let mut headers = parts.headers;
    headers.remove("host");
    headers.remove("connection");
    let response = match state
        .client
        .request(parts.method, format!("{origin}{key}"))
        .headers(headers)
        .body(body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return fallback(),
    };
    let status = response.status();
    let mut headers = response.headers().clone();
    headers.remove("transfer-encoding");
    headers.remove("connection");
    rewrite_location(&mut headers, &origin, &state.public_origin);
    let cacheable = is_get
        && status.is_success()
        && response
            .content_length()
            .is_some_and(|length| length <= 4 * 1024 * 1024)
        && !headers
            .get("content-type")
            .is_some_and(|value| value.as_bytes().starts_with(b"text/event-stream"));
    if cacheable {
        match response.bytes().await {
            Ok(body) => {
                let cached = Cached {
                    status,
                    headers,
                    body,
                };
                state
                    .cache
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(key, cached.clone());
                cached_response(cached)
            }
            Err(_) => fallback(),
        }
    } else {
        let mut output = Response::new(Body::from_stream(response.bytes_stream()));
        *output.status_mut() = status;
        *output.headers_mut() = headers;
        output
    }
}

#[derive(Deserialize)]
struct Ready {
    origin: String,
    base: String,
}

struct Worker {
    child: Child,
    output: tokio::task::JoinHandle<()>,
}

impl Worker {
    async fn start(
        context: &Context,
        executable: &Path,
        ui: &Path,
        origin: &str,
        restarted: bool,
    ) -> Result<(Self, Ready)> {
        let mut arguments = context.worker_args.clone();
        if let Some(index) = arguments.iter().position(|arg| arg == "--port") {
            arguments[index + 1] = "0".into();
        } else {
            arguments.extend(["--port".into(), "0".into()]);
        }
        let mut child = Command::new(executable)
            .args(arguments)
            .current_dir(&context.cwd)
            .env("AGGR_DEV_INTERNAL_WORKER", "1")
            .env("AGGR_DEV_SUPERVISED", "1")
            .env("AGGR_DEV_RESTART", if restarted { "1" } else { "0" })
            .env("AGGR_DEV_PUBLIC_ORIGIN", origin)
            .env("AGGR_UI_DIR", ui)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .context("starting native development worker")?;
        let stdout = child.stdout.take().context("worker stdout")?;
        let (sender, receiver) = oneshot::channel();
        let output = tokio::spawn(async move {
            let mut sender = Some(sender);
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(json) = line
                    .strip_prefix('\u{1e}')
                    .and_then(|line| line.strip_prefix("aggr-ready "))
                {
                    if let Ok(ready) = serde_json::from_str::<Ready>(json)
                        && let Some(sender) = sender.take()
                    {
                        let _ = sender.send(ready);
                    }
                } else {
                    println!("{line}");
                }
            }
        });
        let mut worker = Self { child, output };
        match tokio::time::timeout(Duration::from_secs(300), receiver).await {
            Ok(Ok(origin)) => Ok((worker, origin)),
            _ => {
                worker.stop().await;
                anyhow::bail!("development worker stopped or did not become ready");
            }
        }
    }

    async fn stop(&mut self) {
        if let Some(mut input) = self.child.stdin.take() {
            let _ = input.write_all(b"stop\n").await;
        }
        if tokio::time::timeout(Duration::from_secs(10), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
            let _ = self.child.wait().await;
        }
        self.output.abort();
    }
}

async fn compile(context: &Context, scratch: &Path, generation: usize) -> Result<PathBuf> {
    let jobs = (std::thread::available_parallelism().map_or(1, usize::from) / 2).max(1);
    let mut child = Command::new("cargo")
        .args([
            "build",
            "--locked",
            "--message-format=json-render-diagnostics",
            "--manifest-path",
        ])
        .arg(context.repository.join("Cargo.toml"))
        .args([
            "--bin",
            "aggr-dev-worker",
            "--features",
            "dev-worker",
            "--jobs",
        ])
        .arg(jobs.to_string())
        .current_dir(&context.repository)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .context("building development worker")?;
    let mut lines = BufReader::new(child.stdout.take().context("Cargo build output")?).lines();
    let mut binary = None;
    while let Some(line) = lines.next_line().await? {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
            println!("{line}");
            continue;
        };
        if message["reason"] == "compiler-artifact"
            && message["target"]["name"] == "aggr-dev-worker"
        {
            binary = message["executable"].as_str().map(PathBuf::from);
        }
        if message["reason"] == "compiler-message"
            && let Some(rendered) = message["message"]["rendered"].as_str()
        {
            eprint!("{rendered}");
        }
    }
    ensure!(
        child.wait().await?.success(),
        "Rust development build failed"
    );
    let binary = binary.context("Cargo did not report the development worker executable")?;
    let copy = scratch.join(format!(
        "worker-{generation}{}",
        std::env::consts::EXE_SUFFIX
    ));
    tokio::fs::copy(binary, &copy)
        .await
        .context("retaining last good development worker")?;
    Ok(copy)
}

fn change_kind(root: &Path, path: &Path) -> Option<bool> {
    let relative = path.strip_prefix(root).ok()?;
    let text = relative.to_string_lossy().replace('\\', "/");
    if Path::new(&text).components().any(|part| {
        matches!(
            part.as_os_str().to_str(),
            Some("target" | ".git" | "node_modules" | ".generated")
        )
    }) {
        return None;
    }
    if text.starts_with("web/") || text.starts_with("crates/aggr-ui-build/vendor/") {
        return Some(false);
    }
    if text.starts_with("src/")
        || text.starts_with("crates/")
        || matches!(text.as_ref(), "Cargo.toml" | "Cargo.lock" | "build.rs")
    {
        return Some(true);
    }
    None
}

pub async fn run(arguments: Vec<String>) -> Result<()> {
    let (stop_sender, mut stopping) = oneshot::channel();
    let signal = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = stop_sender.send(());
    });
    let context: Context = if let Ok(json) = std::env::var("AGGR_DEV_CONTEXT") {
        serde_json::from_str(&json).context("reading development launch context")?
    } else {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .context("development repository")?
            .to_owned();
        let mut worker_args = if arguments.is_empty() {
            vec![
                "--config".into(),
                repository
                    .join("examples/aggr.toml")
                    .to_string_lossy()
                    .into_owned(),
                "dev".into(),
            ]
        } else {
            arguments
        };
        if !worker_args.iter().any(|arg| arg == "dev") {
            worker_args.insert(0, "dev".into());
        }
        let port = worker_args
            .windows(2)
            .find(|pair| matches!(pair[0].as_str(), "--port" | "-p"))
            .map(|pair| pair[1].parse::<u16>())
            .transpose()?
            .unwrap_or(7319);
        Context {
            executable: repository
                .join(format!("target/debug/aggr{}", std::env::consts::EXE_SUFFIX)),
            cwd: std::env::current_dir()?,
            repository,
            worker_args,
            public_origin: format!("http://127.0.0.1:{port}"),
            strict_port: false,
        }
    };
    let requested: std::net::SocketAddr = context
        .public_origin
        .strip_prefix("http://")
        .context("development origin must use HTTP")?
        .parse()?;
    let listener = match tokio::net::TcpListener::bind(requested).await {
        Ok(listener) => listener,
        Err(error) if !context.strict_port && error.kind() == std::io::ErrorKind::AddrInUse => {
            tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?
        }
        Err(error) => return Err(error.into()),
    };
    let origin = format!("http://{}", listener.local_addr()?);
    let (sender, mut changes) = mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let _ = sender.send(event);
    })?;
    for name in ["web/src", "web/public", "src", "crates"] {
        watcher.watch(&context.repository.join(name), RecursiveMode::Recursive)?;
    }
    watcher.watch(&context.repository, RecursiveMode::NonRecursive)?;
    watcher.watch(&context.repository.join("web"), RecursiveMode::NonRecursive)?;
    aggr_ui_model::export_types(context.repository.join("web/.generated/types"))?;
    let ui = context.repository.join("web/.generated/native-ui");
    let root = context.repository.clone();
    let out = ui.clone();
    tokio::select! {
        result = tokio::task::spawn_blocking(move || aggr_ui_build::build(&root, &out)) => { result??; },
        _ = &mut stopping => { signal.abort(); return Ok(()); }
    }

    let scratch = tempfile::Builder::new()
        .prefix("aggr-native-dev-")
        .tempdir()?;
    let initial = tokio::select! { result = compile(&context, scratch.path(), 0) => result, _ = &mut stopping => { signal.abort(); return Ok(()); } };
    let executable = initial.or_else(|error| {
        if context.executable.is_file() {
            eprintln!("{error:#}; starting the last compiled worker");
            Ok(context.executable.clone())
        } else {
            Err(error)
        }
    })?;
    let root = context.repository.clone();
    let output = ui.clone();
    tokio::select! {
        result = tokio::task::spawn_blocking(move || aggr_ui_build::build(&root, &output)) => { result??; },
        _ = &mut stopping => { signal.abort(); return Ok(()); }
    }
    let relay_state = Relay {
        public_origin: origin.clone(),
        backend: Arc::default(),
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
        cache: Arc::default(),
    };
    let mut worker = None;
    let serving = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .fallback(relay)
                .with_state(relay_state.clone()),
        )
        .into_future(),
    );
    let work = async {
        let (started, ready) = Worker::start(&context, &executable, &ui, &origin, false).await?;
        *relay_state.backend.write().await = Some(ready.origin);
        worker = Some(started);
        println!("aggr dev: {origin}{}", ready.base);
        let mut last_good = executable;
        let mut generation = 0;
        loop {
            let event = tokio::select! { _ = tokio::signal::ctrl_c() => break, event = changes.recv() => event };
            let Some(Ok(event)) = event else { continue };
            if matches!(event.kind, notify::EventKind::Access(_)) {
                continue;
            }
            let mut kinds: Vec<_> = event
                .paths
                .iter()
                .filter_map(|path| change_kind(&context.repository, path))
                .collect();
            if kinds.is_empty() {
                continue;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
            while let Ok(Ok(event)) = changes.try_recv() {
                if !matches!(event.kind, notify::EventKind::Access(_)) {
                    kinds.extend(
                        event
                            .paths
                            .iter()
                            .filter_map(|path| change_kind(&context.repository, path)),
                    );
                }
            }
            if kinds.contains(&false) {
                let root = context.repository.clone();
                let out = ui.clone();
                match tokio::task::spawn_blocking(move || aggr_ui_build::build(&root, &out)).await {
                    Ok(Ok(_)) => println!("reader rebuilt"),
                    Ok(Err(error)) => {
                        eprintln!("reader build failed; keeping the previous reader: {error:#}")
                    }
                    Err(error) => eprintln!("reader compiler failed: {error}"),
                }
            }
            if kinds.contains(&true) {
                generation += 1;
                let candidate = match compile(&context, scratch.path(), generation).await {
                    Ok(candidate) => candidate,
                    Err(error) => {
                        eprintln!("{error:#}; keeping the running worker");
                        continue;
                    }
                };
                let root = context.repository.clone();
                let output = ui.clone();
                match tokio::task::spawn_blocking(move || aggr_ui_build::build(&root, &output))
                    .await
                {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => {
                        eprintln!(
                            "reader contract build failed; keeping the running worker: {error:#}"
                        );
                        continue;
                    }
                    Err(error) => {
                        eprintln!("reader contract compiler failed: {error}");
                        continue;
                    }
                }
                *relay_state.backend.write().await = None;
                if let Some(active) = &mut worker {
                    active.stop().await;
                }
                match Worker::start(&context, &candidate, &ui, &origin, true).await {
                    Ok((active, ready)) => {
                        worker = Some(active);
                        *relay_state.backend.write().await = Some(ready.origin);
                        if last_good.starts_with(scratch.path()) {
                            let _ = tokio::fs::remove_file(&last_good).await;
                        }
                        last_good = candidate;
                        *relay_state
                            .cache
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) =
                            Responses::default();
                    }
                    Err(error) => {
                        eprintln!("new worker failed; restoring the previous worker: {error:#}");
                        let (active, ready) =
                            Worker::start(&context, &last_good, &ui, &origin, true).await?;
                        worker = Some(active);
                        *relay_state.backend.write().await = Some(ready.origin);
                    }
                }
            }
        }
        Ok(())
    };
    let result: Result<()> =
        tokio::select! { result = work => result, _ = &mut stopping => Ok(()) };
    signal.abort();
    if let Some(active) = &mut worker {
        active.stop().await;
    }
    serving.abort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn relay_preserves_binary_responses_and_serves_last_good_during_handoff() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let bytes = bytes::Bytes::from_static(b"%PDF\0\xff\x80");
        let expected = bytes.clone();
        let app = Router::new().route(
            "/paper.pdf",
            axum::routing::get(move || {
                let bytes = bytes.clone();
                async move {
                    (
                        [("content-type", "application/pdf"), ("x-test", "unchanged")],
                        bytes,
                    )
                }
            }),
        );
        let backend = tokio::spawn(axum::serve(listener, app).into_future());
        let state = Relay {
            public_origin: "http://127.0.0.1:9999".into(),
            backend: Arc::new(RwLock::new(Some(origin))),
            client: reqwest::Client::new(),
            cache: Arc::default(),
        };
        let request = || {
            Request::builder()
                .uri("/paper.pdf")
                .body(Body::empty())
                .unwrap()
        };
        let response = relay(State(state.clone()), request()).await;
        assert_eq!(response.headers()["x-test"], "unchanged");
        assert_eq!(
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
            expected
        );
        *state.backend.write().await = None;
        let response = relay(State(state.clone()), request()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
            expected
        );
        let post = relay(
            State(state.clone()),
            Request::builder()
                .method("POST")
                .uri("/paper.pdf")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(post.status(), StatusCode::SERVICE_UNAVAILABLE);
        let head = relay(
            State(state.clone()),
            Request::builder()
                .method("HEAD")
                .uri("/paper.pdf")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(head.status(), StatusCode::OK);
        assert!(
            axum::body::to_bytes(head.into_body(), usize::MAX)
                .await
                .unwrap()
                .is_empty()
        );
        let unseen = relay(
            State(state),
            Request::builder()
                .uri("/unvisited")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(unseen.status(), StatusCode::SERVICE_UNAVAILABLE);
        backend.abort();
    }

    #[test]
    fn relay_redirects_preserve_relative_and_external_destinations() {
        for (location, expected) in [
            ("../next?q=1", "../next?q=1"),
            (
                "https://publisher.test/article",
                "https://publisher.test/article",
            ),
            (
                "http://127.0.0.1:4444/path?q=1#note",
                "http://127.0.0.1:7319/path?q=1#note",
            ),
            ("//127.0.0.1:4444/path", "http://127.0.0.1:7319/path"),
            ("http://127.0.0.1:44440/path", "http://127.0.0.1:44440/path"),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert("location", location.parse().unwrap());
            rewrite_location(
                &mut headers,
                "http://127.0.0.1:4444",
                "http://127.0.0.1:7319",
            );
            assert_eq!(headers["location"], expected);
        }
    }

    #[test]
    fn ui_changes_do_not_restart_rust_and_generated_outputs_are_ignored() {
        let root = Path::new("/project");
        assert_eq!(
            change_kind(root, Path::new("/project/web/src/App.svelte")),
            Some(false)
        );
        assert_eq!(
            change_kind(
                root,
                Path::new("/project/crates/aggr-lib/src/site/render.rs")
            ),
            Some(true)
        );
        assert_eq!(
            change_kind(
                root,
                Path::new("/project/web/.generated/native-ui/current.json")
            ),
            None
        );
        assert_eq!(
            change_kind(root, Path::new(r"/project/web\src\App.svelte")),
            Some(false)
        );
        assert_eq!(
            change_kind(root, Path::new(r"/project/web\.generated\current.json")),
            None
        );
        assert_eq!(
            change_kind(root, Path::new("/project/target/debug/aggr")),
            None
        );
    }
    #[test]
    fn last_good_response_cache_is_bounded_and_replaces_accounting() {
        let mut cache = Responses::default();
        for index in 0..150 {
            cache.insert(
                index.to_string(),
                Cached {
                    status: StatusCode::OK,
                    headers: HeaderMap::new(),
                    body: bytes::Bytes::from_static(b"page"),
                },
            );
        }
        assert_eq!(cache.entries.len(), 128);
        assert!(!cache.entries.contains_key("0"));
        cache.insert(
            "149".into(),
            Cached {
                status: StatusCode::OK,
                headers: HeaderMap::new(),
                body: bytes::Bytes::from_static(b"new"),
            },
        );
        assert_eq!(cache.bytes, 127 * 4 + 3);
    }
}
