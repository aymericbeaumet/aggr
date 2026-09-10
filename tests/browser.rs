//! Browser contracts against a generated, pinned archive. Run with a local WebDriver:
//! AGGR_WEBDRIVER_URL=http://127.0.0.1:9515 cargo test --test browser -- --ignored

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use assert_cmd::prelude::*;
use fantoccini::{Client, ClientBuilder, Locator};
use futures_util::FutureExt as _;
use serde_json::{Value, json};
use sha1::{Digest as _, Sha1};

struct Fixture {
    _directory: tempfile::TempDir,
    out: PathBuf,
    base: String,
    offline: Arc<AtomicBool>,
    scripts_blocked: Arc<AtomicBool>,
    media: Arc<MediaResponses>,
    app_override: Mutex<Option<String>>,
    stopped: Arc<AtomicBool>,
}

#[derive(Default)]
struct MediaResponses {
    blocked: AtomicBool,
    app_blocked: AtomicBool,
    failed: AtomicBool,
    completed: AtomicUsize,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}

fn git(root: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .current_dir(root)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()?;
    if !output.status.success() {
        bail!("git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(())
}

impl Fixture {
    fn deploy_content_update(&self, index: u32) -> Result<()> {
        let archive = self._directory.path().join(".aggr/data");
        let relative = format!("items/example/2026/09/2026-09-09-story-{index:02}.md");
        std::fs::write(
            archive.join(&relative),
            format!(
                "---\ntitle: Freshly delivered article {index}\nlink: https://publisher.invalid/new-{index}\nsource: example\npublished: 2026-09-09T12:{index:02}:00Z\nfirst_seen: 2026-09-09T12:{index:02}:00Z\ncontent: feed\n---\n\nNewly delivered reading without an app update.\n"
            ),
        )?;
        git(&archive, &["add", &relative])?;
        git(&archive, &["commit", "-qm", "fixture content update"])?;
        self.build()
    }

    fn deploy_app_update(&self) -> Result<()> {
        let root = self._directory.path();
        let config = root.join("aggr.toml");
        std::fs::write(
            &config,
            std::fs::read_to_string(&config)?.replace("Reading room", "Updated reading room"),
        )?;
        self.build()?;
        let manifest_path = self.out.join("updates.json");
        let manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
        let previous = manifest["app_version"]
            .as_str()
            .context("fixture app version")?
            .to_string();
        let next = format!("{previous}-next");
        *self.app_override.lock().expect("fixture app version lock") = Some(next.clone());
        self.apply_app_version(&next)
    }

    fn apply_app_version(&self, next: &str) -> Result<()> {
        let manifest_path = self.out.join("updates.json");
        let mut manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
        let previous = manifest["app_version"]
            .as_str()
            .context("fixture app version")?
            .to_string();
        for entry in walkdir::WalkDir::new(&self.out) {
            let entry = entry?;
            if entry.file_type().is_file()
                && (entry.path().extension().is_some_and(|ext| ext == "html")
                    || entry.file_name() == "sw.js")
            {
                let text = std::fs::read_to_string(entry.path())?;
                std::fs::write(entry.path(), text.replace(&previous, next))?;
            }
        }
        manifest["app_version"] = json!(next);
        std::fs::write(manifest_path, serde_json::to_vec(&manifest)?)?;
        Ok(())
    }

    fn build(&self) -> Result<()> {
        let root = self._directory.path();
        let output = Command::cargo_bin("aggr")?
            .current_dir(root)
            .env_remove("AGGR_CONFIG")
            .env_remove("AGGR_BASE_URL")
            .env_remove("GITHUB_REPOSITORY")
            .env_remove("GITHUB_TOKEN")
            .env_remove("GH_TOKEN")
            .args([
                "build",
                "--data-ref",
                "aggr",
                "--release",
                "--base-url",
                &self.base,
            ])
            .output()?;
        if !output.status.success() {
            bail!(
                "fixture deployment: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        if let Some(version) = self
            .app_override
            .lock()
            .expect("fixture app version lock")
            .clone()
        {
            self.apply_app_version(&version)?;
        }
        Ok(())
    }

    fn new() -> Result<Self> {
        Self::with_pwa(true)
    }

    fn with_pwa(pwa: bool) -> Result<Self> {
        Self::with_base_path(pwa, "reader/")
    }

    fn with_base_path(pwa: bool, base_path: &str) -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let base = format!("http://{}/{base_path}", listener.local_addr()?);
        git(root, &["init", "-q", "-b", "main"])?;
        std::fs::write(
            root.join("aggr.toml"),
            format!(
                "[site]\ntitle='Reading room'\npwa={pwa}\nitems_per_page=3\nmax_age_days=10000\npreferences.offline_items=4\n[[sources]]\nurl='https://publisher.invalid/feed'\nslug='example'\nname='Example'\ncategory='Engineering'\n[[networks]]\nprovider='hackernews'\n[[networks]]\nprovider='reddit'\n"
            ),
        )?;
        git(root, &["add", "aggr.toml"])?;
        git(root, &["commit", "-qm", "fixture config"])?;
        git(root, &["switch", "--orphan", "aggr"])?;
        let items = root.join("items/example/2026/09");
        std::fs::create_dir_all(&items)?;
        let body_image =
            image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(640, 320, |x, y| {
                image::Rgb([(x % 251) as u8, (y % 239) as u8, ((x + y) % 241) as u8])
            }));
        let mut body_bytes = std::io::Cursor::new(Vec::new());
        body_image.write_to(&mut body_bytes, image::ImageFormat::Png)?;
        let body_bytes = body_bytes.into_inner();
        let body_digest = hex::encode(Sha1::digest(&body_bytes));
        let date = chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")?;
        for index in 1..=45 {
            let title = if index == 45 {
                "A long article title that wraps naturally across several lines on a small phone"
                    .to_string()
            } else {
                format!("Article {index}")
            };
            let published = (date + chrono::Duration::hours(index)).to_rfc3339();
            let preview = if index == 45 {
                let image =
                    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(240, 160, |x, y| {
                        image::Rgb([(x / 2) as u8 + 100, (y / 2) as u8 + 80, 80])
                    }));
                let mut bytes = Vec::new();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 75)
                    .encode_image(&image)?;
                let digest = hex::encode(Sha1::digest(&bytes));
                let filename = format!("2026-09-01-story-{index:02}.preview-{}.jpg", &digest[..12]);
                std::fs::write(items.join(&filename), bytes)?;
                format!(
                    "preview:\n  file: {filename}\n  width: 240\n  height: 160\n  alt: Warm geometric illustration\n"
                )
            } else {
                String::new()
            };
            let archived_image = if index == 45 {
                let filename = format!(
                    "2026-09-01-story-{index:02}.image-{}.png",
                    &body_digest[..12]
                );
                std::fs::write(items.join(&filename), &body_bytes)?;
                format!(
                    "images:\n  - source: \"{base}body.png\"\n    original:\n      file: {filename}\n      width: 640\n      height: 320\n    color: \"#315d76\"\n"
                )
            } else {
                String::new()
            };
            let link = match index {
                44 => "https://www.youtube.com/watch?v=M7lc1UVf-VE".to_string(),
                43 => "https://www.twitch.tv/videos/123456789".to_string(),
                42 => "https://vimeo.com/123456789/abc123def4".to_string(),
                41 => format!("{base}document.pdf"),
                _ => format!("https://publisher.invalid/story-{index}"),
            };
            let updated = if index == 44 {
                &published
            } else {
                "2026-09-05T12:00:00Z"
            };
            let markdown = format!(
                "---\ntitle: {title}\nlink: {link}\nsource: example\npublished: {published}\nupdated: {updated}\nfirst_seen: {published}\ncontent: feed\nlabels: [reading, rust]\n{preview}{archived_image}---\n\n* * *\n\nA paragraph with [first link](https://example.invalid/one) and more prose before [a comparison grid](https://example.invalid/two) continues naturally.\n\n```bash\n$ z dotfiles\n$ pwd\n/private/dotfiles\n```\n\n![An article illustration]({base}body.png)\n\nThis entry explores archive topic {index}.\n\n{}\n",
                "Reading comfortably should not change the current page while a deployment arrives.\n\n".repeat(15)
            );
            std::fs::write(
                items.join(format!("2026-09-01-story-{index:02}.md")),
                markdown,
            )?;
        }
        git(root, &["add", "items"])?;
        git(root, &["commit", "-qm", "fixture articles"])?;
        git(root, &["switch", "main"])?;
        let out = root.join("_site");
        let output = Command::cargo_bin("aggr")?
            .current_dir(root)
            .env_remove("AGGR_CONFIG")
            .env_remove("AGGR_BASE_URL")
            .env_remove("GITHUB_REPOSITORY")
            .env_remove("GITHUB_TOKEN")
            .env_remove("GH_TOKEN")
            .args([
                "build",
                "--data-ref",
                "aggr",
                "--release",
                "--base-url",
                &base,
            ])
            .output()?;
        if !output.status.success() {
            bail!("fixture build: {}", String::from_utf8_lossy(&output.stderr));
        }
        std::fs::write(out.join("body.png"), body_bytes)?;
        std::fs::write(out.join("document.pdf"), fixture_pdf())?;
        let offline = Arc::new(AtomicBool::new(false));
        let scripts_blocked = Arc::new(AtomicBool::new(false));
        let media = Arc::new(MediaResponses::default());
        let stopped = Arc::new(AtomicBool::new(false));
        let serving = out.clone();
        let is_offline = offline.clone();
        let block_scripts = scripts_blocked.clone();
        let media_responses = media.clone();
        let is_stopped = stopped.clone();
        listener.set_nonblocking(true)?;
        std::thread::spawn(move || {
            while !is_stopped.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let root = serving.clone();
                        let offline = is_offline.load(Ordering::Relaxed);
                        let scripts_blocked = block_scripts.load(Ordering::Relaxed);
                        let media = media_responses.clone();
                        let stopped = is_stopped.clone();
                        std::thread::spawn(move || {
                            let _ =
                                serve(stream, &root, offline, scripts_blocked, &media, &stopped);
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            _directory: directory,
            out,
            base,
            offline,
            scripts_blocked,
            media,
            app_override: Mutex::new(None),
            stopped,
        })
    }
}

fn fixture_pdf() -> Vec<u8> {
    let text = "BT /F1 20 Tf 24 340 Td (Local PDF fixture) Tj ET\n";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{text}endstream", text.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    ];
    let mut pdf = "%PDF-1.4\n".to_string();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
    }
    let xref = pdf.len();
    pdf.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in offsets {
        pdf.push_str(&format!("{offset:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    pdf.into_bytes()
}

fn serve(
    mut stream: TcpStream,
    root: &Path,
    offline: bool,
    scripts_blocked: bool,
    media: &MediaResponses,
    stopped: &AtomicBool,
) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut buffer = [0; 16384];
    let read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let path = request
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/");
    let media_request = [".pdf", ".png", ".jpg", ".webp", ".wav", ".mp4", ".webm"]
        .iter()
        .any(|extension| path.ends_with(extension))
        || path.ends_with("/media-player.html");
    while (media_request && media.blocked.load(Ordering::Relaxed))
        || (path.contains("/assets/app-") && media.app_blocked.load(Ordering::Relaxed))
    {
        if stopped.load(Ordering::Relaxed) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let accepts_html = request.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("accept") && value.contains("text/html")
        })
    });
    let relative = path
        .strip_prefix("/reader/")
        .or_else(|| path.strip_prefix("/other/"))
        .or_else(|| path.strip_prefix('/'));
    let blocked_worker_update = path.ends_with("/sw.js")
        && root
            .parent()
            .is_some_and(|parent| parent.join(".block-worker-updates").exists());
    let (status, mime, body) = if offline
        || blocked_worker_update
        || (media_request && media.failed.load(Ordering::Relaxed))
    {
        (503, "text/plain", b"offline".to_vec())
    } else if let Some(relative) = relative.filter(|path| !path.split('/').any(|part| part == ".."))
    {
        let file = root.join(if relative.is_empty() {
            "index.html"
        } else {
            relative
        });
        let file = if file.is_dir() {
            file.join("index.html")
        } else {
            file
        };
        let mime = match file
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
        {
            "html" => "text/html; charset=utf-8",
            "js" => "text/javascript",
            "css" => "text/css",
            "json" | "webmanifest" => "application/json",
            "wasm" => "application/wasm",
            "png" => "image/png",
            "jpg" => "image/jpeg",
            "webp" => "image/webp",
            "pdf" => "application/pdf",
            "wav" => "audio/wav",
            "mp4" => "video/mp4",
            "webm" => "video/webm",
            _ => "application/octet-stream",
        };
        match std::fs::read(&file) {
            Ok(bytes) => (200, mime, bytes),
            Err(_) if accepts_html => (
                404,
                "text/html; charset=utf-8",
                std::fs::read(root.join("404.html"))?,
            ),
            Err(_) => (404, "text/plain", b"not found".to_vec()),
        }
    } else {
        (404, "text/plain", b"not found".to_vec())
    };
    let script_policy = if scripts_blocked {
        "Content-Security-Policy: script-src 'none'\r\n"
    } else {
        ""
    };
    write!(
        stream,
        "HTTP/1.1 {status} OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{script_policy}Connection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)?;
    if media_request {
        media.completed.fetch_add(1, Ordering::Relaxed);
    }
    Ok(())
}

async fn wait_for(client: &Client, expression: &str) -> Result<()> {
    let start = Instant::now();
    loop {
        if client
            .execute(&format!("return Boolean({expression})"), vec![])
            .await?
            == json!(true)
        {
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(20) {
            bail!(
                "browser timeout at {}: {expression}",
                client.current_url().await?
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn key(client: &Client, key: &str) -> Result<()> {
    let key = match key {
        "Enter" => "\u{e007}",
        "Tab" => "\u{e004}",
        "Escape" => "\u{e00c}",
        "ArrowLeft" => "\u{e012}",
        "ArrowUp" => "\u{e013}",
        "ArrowRight" => "\u{e014}",
        "ArrowDown" => "\u{e015}",
        key => key,
    };
    client
        .active_element()
        .await?
        .send_keys(key)
        .await
        .with_context(|| format!("sending key {key:?}"))?;
    Ok(())
}

async fn emulate(client: &Client, command: &str, parameters: Value) -> Result<()> {
    let driver = std::env::var("AGGR_WEBDRIVER_URL")?;
    let session = client.session_id().await?.context("browser session")?;
    let response = reqwest::Client::new()
        .post(format!("{driver}/session/{session}/goog/cdp/execute"))
        .json(&json!({"cmd":command,"params":parameters}))
        .send()
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    if !status.is_success() {
        bail!("browser emulation: {body}");
    }
    Ok(())
}

async fn screenshot(client: &Client, name: &str) -> Result<()> {
    let directory = Path::new("target/browser-artifacts");
    std::fs::create_dir_all(directory)?;
    std::fs::write(
        directory.join(format!("{name}.png")),
        client.screenshot().await?,
    )?;
    Ok(())
}

async fn set_offline(client: &Client, offline: bool) -> Result<()> {
    let driver = std::env::var("AGGR_WEBDRIVER_URL")?;
    let session = client.session_id().await?.context("browser session")?;
    let response = reqwest::Client::new()
        .post(format!(
            "{driver}/session/{session}/chromium/network_conditions"
        ))
        .json(&json!({"network_conditions":{
            "offline":offline,"latency":0,"download_throughput":-1,"upload_throughput":-1
        }}))
        .send()
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    if !status.is_success() {
        bail!("setting browser network conditions: {body}");
    }
    Ok(())
}

async fn browser_client() -> Result<Client> {
    browser_client_with_load_strategy("normal").await
}

async fn browser_client_with_load_strategy(strategy: &str) -> Result<Client> {
    let driver = std::env::var("AGGR_WEBDRIVER_URL")
        .context("set AGGR_WEBDRIVER_URL to the local driver")?;
    let mut capabilities = serde_json::Map::new();
    capabilities.insert("browserName".into(), json!("chrome"));
    capabilities.insert("pageLoadStrategy".into(), json!(strategy));
    let mut chrome = json!({"args":["--headless=new", "--no-sandbox", "--disable-dev-shm-usage", "--disable-search-engine-choice-screen"]});
    if let Ok(binary) = std::env::var("AGGR_CHROME_BINARY") {
        chrome["binary"] = json!(binary);
    }
    capabilities.insert("goog:chromeOptions".into(), chrome);
    Ok(
        ClientBuilder::new(hyper_util::client::legacy::connect::HttpConnector::new())
            .capabilities(capabilities)
            .connect(&driver)
            .await?,
    )
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn reader_and_offline_browser_contracts() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::new()?;
    let client = browser_client().await?;
    let result = std::panic::AssertUnwindSafe(run_contracts(&client, &fixture))
        .catch_unwind()
        .await
        .unwrap_or_else(|panic| {
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("browser assertion failed");
            Err(anyhow::anyhow!("{message}"))
        });
    if let Err(error) = &result {
        let _ = screenshot(&client, "failure").await;
        eprintln!("{error:#}; screenshot: target/browser-artifacts/failure.png");
        eprintln!("state: {}", client.execute("return {url:location.href,online:navigator.onLine,swup:!!window.swup,controller:navigator.serviceWorker.controller?.scriptURL,status:document.querySelector('#connection-status')?.textContent,query:document.querySelector('#q')?.value,searchStatus:document.querySelector('#search-status')?.textContent,rows:document.querySelectorAll('#list .row').length,active:document.activeElement?.outerHTML,selected:document.querySelector('#list .row.is-selected')?.dataset.url,scripts:Array.from(document.scripts).map(s=>s.src)}", vec![]).await.unwrap_or(Value::Null));
    }
    client.close().await?;
    result?;
    fixture.scripts_blocked.store(true, Ordering::Relaxed);
    let client = browser_client().await?;
    let result = no_javascript_contracts(&client, &fixture).await;
    if result.is_err() {
        let _ = screenshot(&client, "no-javascript-failure").await;
    }
    client.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn content_and_app_deployments_have_distinct_browser_behavior() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    for (width, installed) in [(1280, false), (390, true)] {
        let fixture = Fixture::with_pwa(installed)?;
        let client = browser_client().await?;
        let result =
            std::panic::AssertUnwindSafe(deployment_contracts(&client, &fixture, width, installed))
                .catch_unwind()
                .await
                .unwrap_or_else(|panic| {
                    let message = panic
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| panic.downcast_ref::<&str>().copied())
                        .unwrap_or("deployment browser assertion failed");
                    Err(anyhow::anyhow!("{message}"))
                });
        if let Err(error) = &result {
            let _ = screenshot(&client, "deployment-failure").await;
            eprintln!("deployment width={width}, installed={installed}: {error:#}");
            eprintln!("deployment state: {}", client.execute_async(r#"
              const done=arguments[arguments.length-1];
              fetch(new URL('updates.json',document.baseURI),{cache:'no-store'}).then(response=>response.json()).then(latest=>done({
                rows:[...document.querySelectorAll('#list .row [data-row-open]')].map(link=>link.textContent),
                empty:document.querySelector('#empty')?.textContent,status:document.querySelector('#search-status')?.textContent,
                versions:window.manifestVersionsSeen,q:document.querySelector('#q')?.value,
                selected:document.querySelector('.row.is-selected [data-row-open]')?.href,
                focused:document.activeElement.outerHTML.slice(0,200),update:document.documentElement.dataset.updateState,
                initialApp:window.AGGR.appVersion,initialContent:window.AGGR.contentVersion,
                latestApp:latest.app_version,latestContent:latest.content_version,
                entryRequests:performance.getEntriesByType('resource').filter(entry=>entry.name.includes('pagefind-entry')).map(entry=>({name:entry.name,size:entry.transferSize}))
              }),error=>done(String(error)));
            "#, vec![]).await.unwrap_or(Value::Null));
        }
        client.close().await?;
        result?;
    }
    Ok(())
}

async fn deployment_contracts(
    client: &Client,
    fixture: &Fixture,
    width: u32,
    installed: bool,
) -> Result<()> {
    accelerate_update_polling(client).await?;
    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":installed}),
    )
    .await?;
    if installed {
        emulate(
            client,
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source":"Object.defineProperty(navigator,'standalone',{value:true,configurable:true})"}),
        )
        .await?;
    }
    client.goto(&fixture.base).await?;
    wait_for(client, "typeof window.swup?.navigate === 'function'").await?;
    if installed {
        wait_for(client, "!!navigator.serviceWorker.controller").await?;
        client
            .execute(
                "window.workerBeforeDeployment = navigator.serviceWorker.controller",
                vec![],
            )
            .await?;
    }
    client
        .execute(
            "window.swup.navigate(arguments[0])",
            vec![json!(format!("{}?q=article", fixture.base))],
        )
        .await?;
    wait_for(client, "document.querySelectorAll('#list .row').length > 0").await?;
    client
        .execute(
            "window.swup.navigate(arguments[0])",
            vec![json!(fixture.base)],
        )
        .await?;
    wait_for(
        client,
        "document.body.dataset.kind === 'river' && !window.swup.navigating",
    )
    .await?;
    client.execute(r#"
      window.deploymentSentinel = 42;
      document.dispatchEvent(new KeyboardEvent('keydown',{key:'j',bubbles:true}));
      document.querySelector('.row.is-selected [data-row-open]').focus({preventScroll:true});
      window.selectedBeforeDeployment = document.querySelector('.row.is-selected [data-row-open]').href;
      window.scrollBeforeDeployment = scrollY;
      window.manifestVersionsSeen = [];
      const fetchOriginal = window.fetch;
      window.fetch = function (...args) {
        return fetchOriginal.apply(this,args).then(response => {
          if (response.url.includes('/updates.json')) response.clone().json().then(value => window.manifestVersionsSeen.push(value.content_version));
          return response;
        });
      };
    "#, vec![]).await?;
    fixture.deploy_content_update(46)?;
    if installed {
        client.execute_async("const done=arguments[arguments.length-1];navigator.serviceWorker.getRegistration().then(registration=>registration.update()).then(()=>done(true),error=>done(error.message))", vec![]).await?;
        wait_for(
            client,
            "navigator.serviceWorker.controller !== window.workerBeforeDeployment",
        )
        .await?;
    }
    wait_for(
        client,
        "document.querySelector('.row [data-row-open]')?.textContent.includes('Freshly delivered article 46')",
    )
    .await?;
    let feed_state = client.execute(r#"
      return {
        sentinel: window.deploymentSentinel,
        selected: document.querySelector('.row.is-selected [data-row-open]')?.href === window.selectedBeforeDeployment,
        focused: document.activeElement?.href === window.selectedBeforeDeployment,
        scroll: scrollY === window.scrollBeforeDeployment,
        update: document.documentElement.dataset.updateState,
        pill: !!document.querySelector('#pwa-refresh') && !document.querySelector('#pwa-refresh').hidden
      };
    "#, vec![]).await?;
    assert_eq!(feed_state["sentinel"], 42, "{feed_state}");
    assert_eq!(feed_state["selected"], true, "{feed_state}");
    assert_eq!(feed_state["focused"], true, "{feed_state}");
    assert_eq!(feed_state["scroll"], true, "{feed_state}");
    assert_ne!(feed_state["update"], "ready", "{feed_state}");
    assert_eq!(feed_state["pill"], false, "{feed_state}");

    client
        .execute(
            "window.swup.navigate(arguments[0])",
            vec![json!(format!("{}?q=Freshly%20delivered", fixture.base))],
        )
        .await?;
    wait_for(
        client,
        "document.querySelector('#list .row [data-row-open]')?.textContent.includes('Freshly delivered article 46')",
    )
    .await?;
    wait_for(client, "!window.swup.navigating").await?;
    client
        .execute(
            r#"
      const row=document.querySelector('#list .row');
      row.classList.add('is-selected');
      const link=row.querySelector('[data-row-open]');
      link.focus({preventScroll:true});
      window.selectedSearchBeforeDeployment=link.href;
      window.searchInputBeforeDeployment=document.querySelector('#q');
    "#,
            vec![],
        )
        .await?;
    fixture.deploy_content_update(47)?;
    wait_for(
        client,
        "document.querySelectorAll('#list .row').length === 2",
    )
    .await?;
    assert_eq!(client.execute(r#"
      return window.deploymentSentinel === 42
        && document.querySelector('#list .row.is-selected [data-row-open]')?.href === window.selectedSearchBeforeDeployment
        && document.activeElement?.href === window.selectedSearchBeforeDeployment
        && document.querySelector('#q') === window.searchInputBeforeDeployment
        && document.querySelector('#q').value === 'Freshly delivered'
        && document.documentElement.dataset.updateState !== 'ready'
        && document.querySelector('#pwa-refresh').hidden;
    "#, vec![]).await?, true, "updating active search must preserve selected result, focus, and the current input");

    client
        .execute(
            "window.swup.navigate(arguments[0])",
            vec![json!(format!(
                "{}items/example/2026-09-01-story-45/",
                fixture.base
            ))],
        )
        .await?;
    wait_for(client, "!!document.querySelector('.body pre')").await?;
    client.execute("history.replaceState(history.state,'',location.pathname+'?reading=1#article');window.articleBeforeDeployment=document.querySelector('article.item');window.scrollTo(0,300)", vec![]).await?;
    let reading_url = client.current_url().await?;
    fixture.deploy_content_update(48)?;
    let content_version: Value =
        serde_json::from_slice(&std::fs::read(fixture.out.join("updates.json"))?)?;
    wait_for(
        client,
        &format!(
            "window.manifestVersionsSeen.includes({})",
            content_version["content_version"]
        ),
    )
    .await?;
    client.execute_async("const done=arguments[arguments.length-1];requestAnimationFrame(()=>requestAnimationFrame(()=>done(true)))", vec![]).await?;
    assert_eq!(client.execute("return window.deploymentSentinel === 42 && document.querySelector('article.item') === window.articleBeforeDeployment && scrollY === 300 && document.documentElement.dataset.updateState !== 'ready' && document.querySelector('#pwa-refresh').hidden", vec![]).await?, true);
    assert_eq!(client.current_url().await?, reading_url);

    fixture.deploy_app_update()?;
    wait_for(client, "document.documentElement.dataset.updateState === 'ready' && !document.querySelector('#pwa-refresh').hidden").await?;
    let app_state = client
        .execute(
            r#"
      const button=document.querySelector('#pwa-refresh'); const box=button.getBoundingClientRect();
      return {text:button.textContent.trim(),width:box.width,height:box.height,
        article:document.querySelector('article.item')===window.articleBeforeDeployment,
        sentinel:window.deploymentSentinel,scroll:scrollY};
    "#,
            vec![],
        )
        .await?;
    assert!(
        app_state["text"]
            .as_str()
            .is_some_and(|text| text.contains("Refresh to update")),
        "{app_state}"
    );
    assert!(
        app_state["width"].as_f64().unwrap_or_default() >= 44.0,
        "{app_state}"
    );
    assert!(
        app_state["height"].as_f64().unwrap_or_default() >= 44.0,
        "{app_state}"
    );
    assert_eq!(app_state["article"], true, "{app_state}");
    assert_eq!(app_state["sentinel"], 42, "{app_state}");
    assert_eq!(app_state["scroll"], 300, "{app_state}");
    if installed {
        client
            .find(Locator::Css("#pwa-refresh"))
            .await?
            .click()
            .await?;
    } else {
        client.refresh().await?;
    }
    wait_for(client, "document.title.includes('Updated reading room') && document.documentElement.dataset.updateState !== 'ready'").await?;
    assert_eq!(client.current_url().await?, reading_url);
    assert_eq!(
        client
            .execute("return window.deploymentSentinel || null", vec![])
            .await?,
        Value::Null
    );
    if installed {
        wait_for(client, "scrollY === 300").await?;
    }
    Ok(())
}

async fn accelerate_update_polling(client: &Client) -> Result<()> {
    emulate(
        client,
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":r#"
      const interval=window.setInterval;
      window.setInterval=function(callback,delay,...args){
        if(delay===60000 || delay===15000){
          return interval.call(this,function(...values){
            if(!window.pauseUpdatePolling) callback(...values);
          },250,...args);
        }
        return interval.call(this,callback,delay,...args);
      };
    "#}),
    )
    .await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn pending_app_release_does_not_block_automatic_feed_updates() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = pending_release_feed_contracts(&client, &fixture).await;
    if let Err(error) = &result {
        let _ = screenshot(&client, "pending-release-feed-failure").await;
        eprintln!("{error:#}");
    }
    client.close().await?;
    result
}

async fn pending_release_feed_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    let stale_feed = std::fs::read_to_string(fixture.out.join("index.html"))?;
    accelerate_update_polling(client).await?;
    client.goto(&fixture.base).await?;
    wait_for(client, "typeof window.swup?.navigate === 'function'").await?;
    client
        .execute(
            r#"
      window.liveFeedSentinel=42;
      document.dispatchEvent(new KeyboardEvent('keydown',{key:'j',bubbles:true}));
      const selected=document.querySelector('.row.is-selected [data-row-open]');
      selected.focus({preventScroll:true});window.liveFeedSelection=selected.href;
    "#,
            vec![],
        )
        .await?;
    fixture.deploy_app_update()?;
    wait_for(client,"document.documentElement.dataset.updateState === 'ready' && !document.querySelector('#pwa-refresh').hidden").await?;
    fixture.deploy_content_update(49)?;
    wait_for(client,"document.querySelector('.row [data-row-open]')?.textContent.includes('Freshly delivered article 49')").await?;
    anyhow::ensure!(client.execute(r#"
      return window.liveFeedSentinel===42 && document.documentElement.dataset.updateState==='ready'
        && !document.querySelector('#pwa-refresh').hidden
        && document.querySelector('.row.is-selected [data-row-open]')?.href===window.liveFeedSelection
        && document.activeElement?.href===window.liveFeedSelection;
    "#,vec![]).await?==true,"content should arrive automatically while the app update waits, preserving selection and focus");
    client
        .execute(
            r#"
      const fetch=window.fetch;
      let hold=true;
      window.fetch=async function(...args){
        const response=await fetch.apply(this,args);
        if(hold && response.url.includes('/updates.json')){
          hold=false;
          window.pauseUpdatePolling=true;
          return new Promise(resolve=>{
            window.releaseManifest=()=>resolve(response);
            window.manifestHeld=true;
          });
        }
        return response;
      };
    "#,
            vec![],
        )
        .await?;
    wait_for(client, "window.manifestHeld === true").await?;
    fixture.deploy_content_update(50)?;
    client
        .execute(
            r#"
      const event=new Event('aggr:build',{cancelable:true});
      window.dispatchEvent(event);
      window.buildEventHandled=event.defaultPrevented;
      window.releaseManifest();
    "#,
            vec![],
        )
        .await?;
    wait_for(client,"document.querySelector('.row [data-row-open]')?.textContent.includes('Freshly delivered article 50')").await?;
    anyhow::ensure!(
        client
            .execute(
                r#"
      return window.buildEventHandled && window.pauseUpdatePolling && window.liveFeedSentinel===42
        && document.documentElement.dataset.updateState==='ready'
        && !document.querySelector('#pwa-refresh').hidden;
    "#,
                vec![]
            )
            .await?
            == true,
        "a build notification during an older manifest request must trigger another check without waiting for the next poll"
    );
    client
        .execute(
            "window.swup.navigate(arguments[0])",
            vec![json!(format!(
                "{}items/example/2026-09-01-story-45/",
                fixture.base
            ))],
        )
        .await?;
    wait_for(
        client,
        "!!document.querySelector('article.item') && !window.swup.navigating",
    )
    .await?;
    client.execute(r#"
      const url=new URL(arguments[0]).pathname;
      window.swup.cache.set(url,{url,html:arguments[1]});
      window.swup.hooks.before('page:view',()=>{
        if(document.querySelector('#aggr-page')?.dataset.kind==='river'){
          window.staleFeedSeen=document.querySelector('.row [data-row-open]')?.href.includes('story-45/');
        }
      });
      window.pauseUpdatePolling=false;
      window.swup.navigate(url);
    "#,vec![json!(fixture.base),json!(stale_feed)]).await?;
    wait_for(client,"document.querySelector('.row [data-row-open]')?.textContent.includes('Freshly delivered article 50')").await?;
    anyhow::ensure!(
        client
            .execute(
                r#"
      return window.staleFeedSeen && window.liveFeedSentinel===42
        && document.documentElement.dataset.updateState==='ready';
    "#,
                vec![]
            )
            .await?
            == true,
        "returning to a stale cached feed must reconcile its displayed content version without a full reload"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn interactive_originals_and_build_time_code_labels() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let archive = fixture._directory.path().join(".aggr/data");
    let relative = "items/example/2026/09/2026-09-09-interactive.md";
    std::fs::write(
        archive.join(relative),
        format!(
            "---\ntitle: Interactive diagrams\nlink: {}interactive.html\nsource: example\npublished: 2026-09-09T12:00:00Z\nfirst_seen: 2026-09-09T12:00:00Z\ncontent: extracted\nextra:\n  content:interactive: true\n---\n\nRetained explanation.\n\n```rust\nfn main() {{ println!(\"hello\"); }}\n```\n",
            fixture.base
        ),
    )?;
    git(&archive, &["add", relative])?;
    git(&archive, &["commit", "-qm", "fixture interactive page"])?;
    fixture.build()?;
    anyhow::ensure!(
        std::fs::read_to_string(
            fixture
                .out
                .join("items/example/2026-09-09-interactive/index.html")
        )?
        .contains("data-interactive-embed"),
        "generated interactive fixture must include its original-link fallback"
    );
    std::fs::write(
        fixture.out.join("interactive.html"),
        r#"<!doctype html><title>Live diagrams</title><canvas width="640" height="320"></canvas><script>let blocked=false;try{parent.document.body}catch(error){blocked=error.name==='SecurityError'}parent.postMessage({interactiveDemo:true,blocked},'*');</script>"#,
    )?;
    let client = browser_client().await?;
    let result = async {
        emulate(&client,"Page.addScriptToEvaluateOnNewDocument",json!({"source":r#"
          addEventListener('message',event=>{if(event.data?.interactiveDemo)window.interactiveReport={...event.data,origin:event.origin}});
          const observer=new MutationObserver(()=>{
            const box=document.querySelector('.interactive-frame');
            if(!box?.querySelector('iframe'))return;
            const r=box.getBoundingClientRect();
            window.interactiveInitialGeometry={width:r.width,height:r.height};
            observer.disconnect();
          });
          observer.observe(document,{childList:true,subtree:true});
        "#})).await?;
        for width in [1280,390] {
            emulate(&client,"Emulation.setDeviceMetricsOverride",json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":width<600})).await?;
            client.goto(&format!("{}items/example/2026-09-09-interactive/",fixture.base)).await?;
            wait_for(&client,"window.interactiveReport?.blocked===true && document.querySelector('[data-interactive-embed]').hidden").await?;
            let state=client.execute(r#"
              const box=document.querySelector('.interactive-frame'),frame=box.querySelector('iframe'),r=box.getBoundingClientRect(),code=document.querySelector('.code-snippet');
              return {width:r.width,height:r.height,before:window.interactiveInitialGeometry,focused:document.activeElement===frame,frames:box.querySelectorAll('iframe').length,sandbox:frame.getAttribute('sandbox'),referrer:frame.referrerPolicy,origin:window.interactiveReport.origin,language:code.dataset.language,label:getComputedStyle(code,'::before').content,labelColor:getComputedStyle(code,'::before').color,source:code.textContent,highlighted:!!code.querySelector('[class*=syntax-keyword],[class*=syntax-storage]'),overflow:document.documentElement.scrollWidth>document.documentElement.clientWidth+1};
            "#,vec![]).await?;
            anyhow::ensure!(state["width"]==state["before"]["width"] && state["height"]==state["before"]["height"] && state["overflow"]==false,"automatic interactive loading preserves desktop/mobile geometry: {state}");
            anyhow::ensure!(state["focused"]==false,"automatic interactive loading must not move keyboard focus: {state}");
            anyhow::ensure!(state["sandbox"]=="allow-scripts" && state["referrer"]=="no-referrer" && state["origin"]=="null" && state["frames"]==1,"interactive source keeps an opaque origin and minimal permissions: {state}");
            anyhow::ensure!(state["language"]=="Rust" && state["label"]=="\"Rust\"" && state["highlighted"]==true && state["source"]=="fn main() { println!(\"hello\"); }\n","build-time labels and syntax colors preserve copied code: {state}");
        }
        Ok(())
    }.await;
    if result.is_err() {
        let _ = screenshot(&client, "interactive-failure").await;
        eprintln!("interactive state: {}", client.execute("return {url:location.href,title:document.title,swup:!!window.swup,link:document.querySelector('[data-interactive-embed]')?.outerHTML,body:document.body.innerText.slice(0,300),scripts:[...document.scripts].map(s=>s.src)}",vec![]).await.unwrap_or(Value::Null));
    }
    client.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn recommendation_cards_and_navigation_layout() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        for width in [1280, 390] {
            emulate(&client, "Emulation.setDeviceMetricsOverride", json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":width<600})).await?;
            client.goto(&format!("{}items/example/2026-09-01-story-20/", fixture.base)).await?;
            wait_for(&client, "document.querySelectorAll('.article-more-section').length===2").await?;
            let layout = client.execute(r#"
              const sections=[...document.querySelectorAll('.article-more-section')];
              const cards=sections.map(section=>section.querySelector('.article-more-card'));
              cards[0].querySelector('.preview-media')?.remove();
              cards[0].querySelector('.title').textContent='Short title';
              cards[1].querySelector('.title').textContent='A much longer recommendation title that wraps onto several lines and needs more natural reading space';
              const boxes=cards.map(card=>{const r=card.getBoundingClientRect();return {top:r.top,bottom:r.bottom,height:r.height}});
              const unusedHeight=cards[0].querySelector('.row-content').getBoundingClientRect().height-cards[0].querySelector('.row-copy').getBoundingClientRect().height;
              return {boxes,unusedHeight,headingMargin:parseFloat(getComputedStyle(document.querySelector('.itemhead')).marginBottom),padding:cards.map(card=>getComputedStyle(card).paddingTop)};
            "#, vec![]).await?;
            let first=&layout["boxes"][0];
            let second=&layout["boxes"][1];
            anyhow::ensure!(second["top"].as_f64()>first["bottom"].as_f64(), "recommendations stack vertically at every viewport width: {layout}");
            anyhow::ensure!(layout["unusedHeight"].as_f64().unwrap().abs()<1.0, "text-only recommendations fit their content without reserving absent previews: {layout}");
            anyhow::ensure!(layout["headingMargin"].as_f64().unwrap()>=if width>600 {36.0} else {28.0}, "article header needs a little breathing room: {layout}");
            anyhow::ensure!(layout["padding"].as_array().unwrap().iter().all(|value|value=="12.8px"), "card padding stays compact: {layout}");
            wait_for(&client, "typeof window.swup?.navigate==='function'").await?;
            for index in [0, 1] {
                let point = client.execute(r#"
                  const card=document.querySelectorAll('.article-more-card')[arguments[0]];
                  card.scrollIntoView({block:'center'});
                  const r=card.getBoundingClientRect();
                  return {x:r.left+4,y:r.top+4,background:getComputedStyle(card).backgroundColor};
                "#, vec![json!(index)]).await?;
                emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mouseMoved","x":point["x"],"y":point["y"]})).await?;
                let hover=client.execute(r#"
                  const card=document.querySelectorAll('.article-more-card')[arguments[0]];
                  return {background:getComputedStyle(card).backgroundColor,cursor:getComputedStyle(card).cursor,decoration:getComputedStyle(card.querySelector('.title')).textDecorationLine};
                "#,vec![json!(index)]).await?;
                anyhow::ensure!(hover["background"]==point["background"] && hover["cursor"]=="pointer" && hover["decoration"]=="underline", "card space underlines the title without changing its surface: {hover}");
                let clicks=client.execute(r#"
                  const card=document.querySelectorAll('.article-more-card')[arguments[0]],title=card.querySelector('.title'),forwarded=[];
                  const capture=event=>{forwarded.push({button:event.button,ctrl:event.ctrlKey,shift:event.shiftKey});event.preventDefault();event.stopPropagation()};
                  title.addEventListener('click',capture);
                  card.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true,ctrlKey:true,shiftKey:true}));
                  card.dispatchEvent(new MouseEvent('auxclick',{bubbles:true,cancelable:true,button:1}));
                  const range=document.createRange();range.selectNodeContents(title);getSelection().addRange(range);
                  card.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true}));
                  getSelection().removeAllRanges();
                  title.removeEventListener('click',capture);
                  return forwarded;
                "#,vec![json!(index)]).await?;
                anyhow::ensure!(clicks==json!([{"button":0,"ctrl":true,"shift":true},{"button":1,"ctrl":false,"shift":false}]), "card clicks preserve modifiers and leave text selection alone: {clicks}");
            }
            let destination=client.execute("const card=document.querySelector('.article-more-card');const href=card.querySelector('.title').href;card.click();return href",vec![]).await?;
            wait_for(&client,&format!("location.href==={destination}")).await?;
            client.goto(&format!("{}categories/", fixture.base)).await?;
            let nav=client.execute(r#"
              const links=[...document.querySelectorAll('.nav .menu-link')];
              return {regular:links.every(link=>getComputedStyle(link).fontWeight==='400'),prominent:links.every(link=>getComputedStyle(link).opacity==='1'),selected:getComputedStyle(document.querySelector('.nav a[aria-current]')).textDecorationLine,pipe:document.querySelector('.nav-primary .nav-separator')?.textContent};
            "#,vec![]).await?;
            anyhow::ensure!(nav["regular"]==true && nav["prominent"]==true && nav["selected"]=="underline" && nav["pipe"]=="|", "navigation uses regular prominent labels and a selected underline: {nav}");
        }
        Ok(())
    }.await;
    client.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn mobile_tabs_and_instant_cached_navigation() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_base_path(false, "reader/")?;
    let client = browser_client().await?;
    let result = async {
        for (width, height) in [(390, 844), (360, 780)] {
            emulate(&client, "Emulation.setDeviceMetricsOverride", json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":true})).await?;
            client.goto(&fixture.base).await?;
            wait_for(&client, "!!window.swup && document.querySelector('.search-command')").await?;
            let layout = client.execute(r#"
              const bar=document.querySelector('.mobile-tabs'),r=bar.getBoundingClientRect();
              const visible=element=>!!element.getClientRects().length&&getComputedStyle(element).visibility!=='hidden';
              return {bottom:r.bottom,height:r.height,viewport:innerHeight,position:getComputedStyle(bar).position,
                top:[...document.querySelectorAll('.nav a')].filter(visible).map(a=>a.textContent.trim()),
                tabs:[...bar.querySelectorAll('a')].filter(visible).map(a=>({label:a.textContent.trim(),height:a.getBoundingClientRect().height,current:a.getAttribute('aria-current')})),
                padding:parseFloat(getComputedStyle(document.querySelector('.main')).paddingBottom),footer:visible(document.querySelector('.footer'))};
            "#,vec![]).await?;
            anyhow::ensure!(layout["position"]=="fixed" && (layout["bottom"].as_f64().unwrap()-layout["viewport"].as_f64().unwrap()).abs()<1.0,"tabs meet the viewport bottom: {layout}");
            anyhow::ensure!(layout["top"].as_array().unwrap().len()==2 && layout["top"][1]=="aggr.toml ↗" && layout["footer"]==false,"mobile header has only brand and config: {layout}");
            let tabs=layout["tabs"].as_array().unwrap();
            anyhow::ensure!(tabs.iter().map(|tab|tab["label"].as_str().unwrap()).collect::<Vec<_>>()==["feed","search","browse","preferences"] && tabs.iter().all(|tab|tab["height"].as_f64().unwrap()>=44.0) && tabs[0]["current"]=="page","four accessible tabs with an active feed: {layout}");
            anyhow::ensure!((layout["padding"].as_f64().unwrap()-layout["height"].as_f64().unwrap()-12.0).abs()<1.0,"content clears the measured bar with one compact gap: {layout}");
            screenshot(&client, &format!("mobile-tabs-{width}")).await?;
            client.execute("scrollTo(0,document.documentElement.scrollHeight);document.documentElement.style.setProperty('--mobile-safe-area','32px')",vec![]).await?;
            wait_for(&client,"Math.abs(parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--bottom-nav-offset'))-document.querySelector('.mobile-tabs').getBoundingClientRect().height)<1").await?;
            let inset=client.execute(r#"
              const bar=document.querySelector('.mobile-tabs'),r=bar.getBoundingClientRect();
              return {bottom:r.bottom,viewport:innerHeight,height:r.height,padding:parseFloat(getComputedStyle(document.querySelector('.main')).paddingBottom),safe:parseFloat(getComputedStyle(bar).paddingBottom)};
            "#,vec![]).await?;
            anyhow::ensure!(inset["safe"]==32 && (inset["height"].as_f64().unwrap()-layout["height"].as_f64().unwrap()-32.0).abs()<1.0 && (inset["bottom"].as_f64().unwrap()-inset["viewport"].as_f64().unwrap()).abs()<1.0,"scrolling keeps the bar docked and safe area is counted once: {inset}");
            anyhow::ensure!((inset["padding"].as_f64().unwrap()-inset["height"].as_f64().unwrap()-12.0).abs()<1.0,"home-indicator padding is not duplicated in content: {inset}");
            let keyboard=client.execute_async(r#"
              const done=arguments[arguments.length-1],bar=document.querySelector('.mobile-tabs'),input=document.querySelector('#q'),viewport=window.visualViewport;
              const height=bar.getBoundingClientRect().height,padding=getComputedStyle(document.querySelector('.main')).paddingBottom;
              input.focus();
              Object.defineProperty(viewport,'height',{configurable:true,value:innerHeight-300});
              viewport.dispatchEvent(new Event('resize'));
              const hidden=getComputedStyle(bar).visibility==='hidden';
              input.blur();
              delete viewport.height;
              queueMicrotask(()=>done({hidden,restored:getComputedStyle(bar).visibility==='visible',stable:bar.getBoundingClientRect().height===height&&getComputedStyle(document.querySelector('.main')).paddingBottom===padding}));
            "#,vec![]).await?;
            anyhow::ensure!(keyboard==json!({"hidden":true,"restored":true,"stable":true}),"keyboard hides controls without shifting content or floating above keys: {keyboard}");
            let reselected=client.execute_async(r#"
              const done=arguments[arguments.length-1],bar=document.querySelector('.mobile-tabs'),feed=bar.querySelector('a[data-route=""]'),input=document.querySelector('#q');
              input.focus();
              document.querySelector('.main').style.minHeight='1600px';
              scrollTo(0,200);
              let visits=0;
              const off=window.swup.hooks.on('visit:start',()=>visits++);
              feed.click();
              queueMicrotask(()=>{off();done({top:scrollY,editing:document.activeElement===input,visits,href:location.href,expected:feed.href});document.querySelector('.main').style.removeProperty('min-height')});
            "#,vec![]).await?;
            anyhow::ensure!(reselected["top"]==0 && reselected["editing"]==false && reselected["visits"]==0 && reselected["href"]==reselected["expected"],"reselecting Feed returns to its top without opening search or navigating again: {reselected}");
            let tab_point=client.execute(r#"const r=document.querySelector('.mobile-tabs a[data-route="browse/"]').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}"#,vec![]).await?;
            let before_press=client.execute(r#"const tab=document.querySelector('.mobile-tabs a[data-route="browse/"]');const r=tab.getBoundingClientRect();return {background:getComputedStyle(tab).backgroundColor,top:r.top,height:r.height}"#,vec![]).await?;
            emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mousePressed","button":"left","buttons":1,"clickCount":1,"x":tab_point["x"],"y":tab_point["y"]})).await?;
            let pressed=client.execute(r#"const tab=document.querySelector('.mobile-tabs a[data-route="browse/"]');const r=tab.getBoundingClientRect();return {background:getComputedStyle(tab).backgroundColor,top:r.top,height:r.height,transition:getComputedStyle(tab).transitionDuration,opacity:getComputedStyle(tab).opacity}"#,vec![]).await?;
            emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mouseReleased","button":"left","buttons":0,"clickCount":1,"x":1,"y":1})).await?;
            anyhow::ensure!(pressed["background"]!=before_press["background"] && pressed["top"]==before_press["top"] && pressed["height"]==before_press["height"] && pressed["transition"]=="0s" && pressed["opacity"]=="1","tabs give immediate pressed feedback without fading labels or shifting layout: {pressed}");
            let navigation=client.execute_async(r#"
              const done=arguments[arguments.length-1];
              (async()=>{
                const until=async check=>{const start=performance.now();while(!check()){if(performance.now()-start>10000)throw Error('waiting for cached navigation');await new Promise(requestAnimationFrame)}};
                let animationStarts=0;const durations=[];
                const off=window.swup.hooks.on('animation:in:start',()=>animationStarts++);
                for(const route of ['browse/','preferences/','']){
                  const url=new URL(route,document.baseURI);
                  await until(()=>!window.swup.navigating&&window.swup.cache.has(url.pathname));
                  const started=performance.now();
                  document.querySelector(`.mobile-tabs a[data-route="${route}"]`).click();
                  await until(()=>!window.swup.navigating&&location.pathname===url.pathname&&document.querySelector('.mobile-tabs a[aria-current]')?.dataset.route===route);
                  durations.push({route,ms:performance.now()-started});
                  if(document.querySelectorAll('.mobile-tabs a[aria-current]').length!==1)throw Error('ambiguous active tab');
                  if(route==='browse/' && !['categories','sources','tags'].every(kind=>document.querySelector('.browse-group-'+kind+' .browse-entry-link')))throw Error('Browse must expose every populated directory');
                }
                off();
                return {animationStarts,durations,cacheSize:window.swup.cache.size};
              })().then(done,error=>done({error:String(error)}));
            "#,vec![]).await?;
            anyhow::ensure!(navigation.get("error").is_none() && navigation["animationStarts"]==0 && navigation["cacheSize"].as_u64().unwrap()<=32,"cached tab navigation skips all animation frames: {navigation}");
            eprintln!("mobile cached navigation ({width}px): {navigation}");
            client.find(Locator::Css(".mobile-tabs [data-search-action]")).await?.click().await?;
            wait_for(&client,"document.activeElement?.id==='q' && document.querySelector('.mobile-tabs [aria-current]')?.hasAttribute('data-search-action')").await?;
            client.find(Locator::Css("#q")).await?.send_keys("category:engineering").await?;
            wait_for(&client,"new URL(location.href).searchParams.get('q')==='category:engineering'").await?;
            let search_reselected=client.execute_async(r#"
              const done=arguments[arguments.length-1],input=document.querySelector('#q'),tab=document.querySelector('.mobile-tabs [data-search-action]');
              input.setSelectionRange(9,12);
              let visits=0;const off=window.swup.hooks.on('visit:start',()=>visits++);
              tab.click();
              queueMicrotask(()=>{off();done({query:input.value,selection:[input.selectionStart,input.selectionEnd],focused:document.activeElement===input,visits,active:document.querySelectorAll('.mobile-tabs [aria-current]').length,hrefQuery:new URL(tab.href).searchParams.get('q')})});
            "#,vec![]).await?;
            anyhow::ensure!(search_reselected==json!({"query":"category:engineering","selection":[9,12],"focused":true,"visits":0,"active":1,"hrefQuery":"category:engineering"}),"Search reselect preserves the query, caret, and native link destination without navigating: {search_reselected}");
            key(&client,"Escape").await?;
            wait_for(&client,"document.activeElement?.id!=='q' && document.querySelector('.mobile-tabs [aria-current]')?.hasAttribute('data-search-action')").await?;
            client.execute("const input=document.querySelector('#q');input.value='';input.dispatchEvent(new Event('input',{bubbles:true}));input.blur()",vec![]).await?;
            wait_for(&client,"!new URL(location.href).searchParams.has('q') && document.querySelector('.mobile-tabs [aria-current]')?.hasAttribute('data-feed-action')").await?;
            client.find(Locator::Css(".mobile-tabs [data-route='browse/']")).await?.click().await?;
            wait_for(&client,"document.body.dataset.kind==='browse' && !window.swup.navigating").await?;
            client.find(Locator::Css(".mobile-tabs [data-search-action]")).await?.click().await?;
            wait_for(&client,"location.pathname==='/reader/' && document.activeElement?.id==='q' && document.querySelector('.mobile-tabs [aria-current]')?.hasAttribute('data-search-action')").await?;
        }
        Ok(())
    }.await;
    if result.is_err() {
        let _ = screenshot(&client, "mobile-tabs-failure").await;
    }
    client.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn mobile_feed_rows_keep_metadata_readable_without_thumbnail_indentation() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_base_path(false, "reader/")?;
    let client = browser_client().await?;
    let result = async {
        for width in [390, 320] {
            emulate(&client, "Emulation.setDeviceMetricsOverride", json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":true})).await?;
            client.goto(&fixture.base).await?;
            wait_for(&client,"!!window.swup && document.querySelector('.row .preview-media')").await?;
            let layout=client.execute(r#"
              const rows=[...document.querySelectorAll('.row')],row=rows[0],copy=row.querySelector('.row-content'),title=row.querySelector('.title'),meta=row.querySelector('.meta'),preview=row.querySelector('.preview-media');
              const source=meta.querySelector('.meta-field:has(.domain)'),fields=[...meta.children];
              const textOnly=rows.find(row=>!row.querySelector('.preview-media'));
              const box=node=>node.getBoundingClientRect();
              return {overflow:document.documentElement.scrollWidth>innerWidth,
                rank:rows.every(row=>getComputedStyle(row.querySelector('.rank')).display==='none'),
                metadataFullWidth:Math.abs(box(meta).width-box(copy).width)<1,
                sharedAxis:Math.abs(box(meta).left-box(title).left)<1,
                sourceSeparate:fields.slice(1).every(field=>box(field).top>=box(source).bottom-1),
                fieldsVisible:fields.every(field=>box(field).height>0 && getComputedStyle(field).visibility!=='hidden'),
                separators:fields.every(field=>getComputedStyle(field,'::before').content==='none'),
                preview:{width:box(preview).width,height:box(preview).height,top:box(preview).top-title.getBoundingClientRect().top,right:box(preview).right-box(copy).right},
                textOnlyMinHeight:getComputedStyle(textOnly.querySelector('.row-content')).minHeight,
                rowTargets:rows.every(row=>box(row).height>=44),
                padding:parseFloat(getComputedStyle(row).paddingTop),
                titleSize:parseFloat(getComputedStyle(title).fontSize),
                metaSize:parseFloat(getComputedStyle(meta).fontSize)};
            "#,vec![]).await?;
            anyhow::ensure!(layout["overflow"]==false && layout["rank"]==true && layout["metadataFullWidth"]==true && layout["sharedAxis"]==true,"mobile text uses the full row width without a rank or preview column below the title: {layout}");
            anyhow::ensure!(layout["sourceSeparate"]==true && layout["fieldsVisible"]==true && layout["separators"]==true,"source and secondary metadata form a clear hierarchy without losing fields or wrapping leading dots: {layout}");
            anyhow::ensure!(layout["preview"]==json!({"width":48,"height":48,"top":0,"right":0}) && layout["textOnlyMinHeight"]=="0px","previews reserve a stable square only where present: {layout}");
            anyhow::ensure!(layout["rowTargets"]==true && layout["padding"].as_f64().unwrap()>=10.0 && layout["titleSize"].as_f64().unwrap()>=16.0 && layout["metaSize"].as_f64().unwrap()>=13.0,"mobile rows retain readable text and a comfortable full-row target: {layout}");
            let hidden=client.execute("document.documentElement.dataset.thumbnails='hide';const row=document.querySelector('.row'),full=row.querySelector('.row-content').getBoundingClientRect().width,title=row.querySelector('.title').getBoundingClientRect().width;const hidden=getComputedStyle(row.querySelector('.preview-media')).display==='none';delete document.documentElement.dataset.thumbnails;return {hidden,reclaimed:Math.abs(full-title)<1}",vec![]).await?;
            anyhow::ensure!(hidden==json!({"hidden":true,"reclaimed":true}),"hiding previews also removes their column and gap: {hidden}");
            screenshot(&client,&format!("mobile-feed-ergonomics-{width}")).await?;
            let target=client.execute("const row=document.querySelector('.row');const box=row.getBoundingClientRect();return {x:box.left+4,y:box.bottom-4,url:row.querySelector('[data-row-open]').href}",vec![]).await?;
            emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mousePressed","button":"left","buttons":1,"clickCount":1,"x":target["x"],"y":target["y"]})).await?;
            emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mouseReleased","button":"left","buttons":0,"clickCount":1,"x":target["x"],"y":target["y"]})).await?;
            wait_for(&client,&format!("location.href==={}",target["url"])).await?;
        }
        Ok(())
    }.await;
    if result.is_err() {
        let _ = screenshot(&client, "mobile-feed-ergonomics-failure").await;
    }
    client.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn contextual_source_completion_and_item_types() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let root = fixture._directory.path();
    let config = root.join("aggr.toml");
    std::fs::write(
        &config,
        format!(
            "{}\n[[sources]]\nurl='https://open.spotify.com/show/opaque123'\nslug='open-spotify-com-show-opaque123'\nname='Underscore_'\ncategory='News'\n",
            std::fs::read_to_string(&config)?
        ),
    )?;
    let archive = root.join(".aggr/data");
    let directory = archive.join("items/open-spotify-com-show-opaque123/2026/09");
    std::fs::create_dir_all(&directory)?;
    std::fs::write(
        directory.join("2026-09-01-podcast.md"),
        "---\ntitle: A podcast episode\nlink: https://open.spotify.com/episode/episode123\nsource: open-spotify-com-show-opaque123\npublished: 2026-09-01T12:00:00Z\nfirst_seen: 2026-09-01T12:00:00Z\ncontent: feed\n---\n\nA discussion about technology.\n",
    )?;
    git(&archive, &["add", "items"])?;
    git(&archive, &["commit", "-qm", "fixture podcast source"])?;
    fixture.build()?;
    let client = browser_client().await?;
    let result = async {
        client.goto(&fixture.base).await?;
        wait_for(&client, "!!document.querySelector('.search-input-line')").await?;
        for (query, label, count) in [
            ("category:engineering source:", "Example", 45),
            ("category:news source:", "Underscore_", 1),
            ("type:podcast source:", "Underscore_", 1),
        ] {
            client.execute("const q=document.querySelector('#q');q.focus();q.value=arguments[0];q.setSelectionRange(q.value.length,q.value.length);q.dispatchEvent(new Event('input',{bubbles:true}))",vec![json!(query)]).await?;
            wait_for(&client,&format!("document.querySelectorAll('.search-completion').length===1 && document.querySelector('.completion-label')?.textContent==={}",json!(label))).await?;
            anyhow::ensure!(client.execute("return document.querySelector('.search-completion small').textContent",vec![]).await?==format!("source · {count}"),"source counts reflect the other active clauses");
        }
        key(&client,"Enter").await?;
        wait_for(&client,"document.querySelector('#q').value.includes('source:\"Underscore_\"') && !document.querySelector('.search-completions') && document.querySelector('#search-status')?.textContent==='1 article'").await?;
        anyhow::ensure!(client.execute("return document.querySelector('.search-results .title').textContent",vec![]).await?=="A podcast episode","Enter selects the named show");
        anyhow::ensure!(client.execute("return document.querySelector('.search-results .source-resolved').textContent",vec![]).await?=="spotify.com/underscore","podcast metadata uses a readable profile label");
        search_query(&client,"type:document",1).await?;
        anyhow::ensure!(client.execute("return document.querySelector('.search-results .u-bookmark-of').href.endsWith('document.pdf')",vec![]).await?==true,"PDFs are searchable as documents");
        Ok(())
    }.await;
    client.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn selected_feed_external_shortcuts() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        for search in [false, true] {
            client.goto(&fixture.base).await?;
            wait_for(&client, "typeof window.swup?.navigate === 'function' && !!document.querySelector('.search-command') && !!document.querySelector('.row.is-selected')").await?;
            if search {
                client.find(Locator::Css("#q")).await?.send_keys("source:example").await?;
                wait_for(&client, "!!document.querySelector('.search-results .row.is-selected')").await?;
                key(&client, "Escape").await?;
            }
            key(&client, "j").await?;
            let targets = client.execute(r#"
              const root=document.querySelector('.search-results') || document.querySelector('[data-static-feed]');
              const row=root.querySelector('.row.is-selected'), original=row.querySelector('.u-bookmark-of').href;
              const networks=window.AGGR.discussions.filter(network=>network.shortcut);
              window.__opened=[];window.open=url=>{window.__opened.push(url);return {}};
              return {keys:['O',...networks.map(network=>network.shortcut)],urls:[original,...networks.map(network=>{
                const found=[...row.querySelectorAll('.discussion')].find(link=>link.dataset.discussion===network.name);
                return found?.href || network.url.split('{url}').join(encodeURIComponent(original)).split('{title}').join(encodeURIComponent(row.querySelector('.p-name').textContent));
              })]};
            "#,vec![]).await?;
            for value in targets["keys"].as_array().context("shortcut keys")? {
                key(&client, value.as_str().context("shortcut key")?).await?;
            }
            let opened=client.execute("return window.__opened",vec![]).await?;
            anyhow::ensure!(opened==targets["urls"], "external shortcuts use the selected row on static and searched feeds: {opened}, expected {targets}");
            key(&client,"\u{e009}k\u{e000}").await?;
            wait_for(&client,"document.activeElement?.id==='q'").await?;
            key(&client,"O").await?;
            anyhow::ensure!(client.execute("return window.__opened",vec![]).await?==opened,"typing in search must not open selected articles");
        }
        Ok(())
    }.await;
    client.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn feed_boundary_shortcuts_select_rows_and_article_boundaries_scroll() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        for search in [false, true] {
            client.goto(&fixture.base).await?;
            wait_for(&client, "typeof window.swup?.navigate === 'function' && !!document.querySelector('.search-command') && !!document.querySelector('.row.is-selected')").await?;
            if search {
                client.find(Locator::Css("#q")).await?.send_keys("source:example").await?;
                wait_for(&client, "!!document.querySelector('.search-results .row.is-selected')").await?;
                key(&client, "Escape").await?;
            }
            for (keys, edge) in [("G", "last"), ("gg", "first")] {
                key(&client, keys).await?;
                let state = client.execute("const root=document.querySelector('.search-results') || document.querySelector('[data-static-feed]');const rows=[...root.querySelectorAll('.row:not([hidden])')];const expected=arguments[0]==='first'?rows[0]:rows.at(-1);return {count:rows.length,selected:expected.classList.contains('is-selected'),focused:expected.querySelector('[data-row-open]')===document.activeElement}", vec![json!(edge)]).await?;
                anyhow::ensure!(state["count"].as_u64().unwrap_or(0)>1 && state["selected"]==true && state["focused"]==true, "{keys} must select and focus the {edge} visible feed row (search={search}): {state}");
                let boundary = if edge == "first" { "scrollY === 0" } else { "Math.abs(document.documentElement.scrollHeight - innerHeight - scrollY) < 2" };
                anyhow::ensure!(client.execute(&format!("return {boundary}"), vec![]).await? == true, "{keys} must also reach the {edge} scroll boundary (search={search})");
            }
        }
        client.goto(&format!("{}items/example/2026-09-01-story-45/", fixture.base)).await?;
        wait_for(&client, "typeof window.swup?.navigate === 'function' && !!document.querySelector('article.item')").await?;
        client.execute("document.querySelector('.body').style.minHeight='250vh'",vec![]).await?;
        key(&client,"G").await?;
        wait_for(&client,"scrollY>0 && Math.abs(document.documentElement.scrollHeight-innerHeight-scrollY)<2").await?;
        key(&client,"gg").await?;
        wait_for(&client,"scrollY===0").await?;
        Ok(())
    }.await;
    client.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn prefetch_reserves_capacity_for_pointer_intent() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        emulate(&client, "Page.addScriptToEvaluateOnNewDocument", json!({"source": r#"
          window.prefetchProbe={started:[],release:[]};
          const original=window.fetch;
          window.fetch=function(input,options){
            if(options?.priority!=='low')return original.call(this,input,options);
            const url=new URL(typeof input==='string'?input:input.url,document.baseURI).href;
            window.prefetchProbe.started.push(url);
            return new Promise(resolve=>window.prefetchProbe.release.push(resolve)).then(()=>original.call(this,input,options));
          };
        "#})).await?;
        client.goto(&fixture.base).await?;
        wait_for(&client,"window.prefetchProbe.started.length>0").await?;
        let started=client.execute("const probe=window.prefetchProbe,idle=probe.started.length;const link=[...document.querySelectorAll('[data-row-open]')].at(-1);link.dispatchEvent(new PointerEvent('pointerover',{bubbles:true}));return {idle,target:link.href}",vec![]).await?;
        wait_for(&client,"window.prefetchProbe.started.length===2").await?;
        let requests=client.execute("return window.prefetchProbe.started",vec![]).await?;
        anyhow::ensure!(started["idle"]==1 && requests[1]==started["target"],"one idle request leaves room for immediate pointer intent: started={started}, requests={requests}");
        client.execute("window.prefetchProbe.release.forEach(resolve=>resolve())",vec![]).await?;
        Ok(())
    }.await;
    client.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn mobile_search_pagination_uses_compact_inline_controls() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        emulate(&client, "Page.addScriptToEvaluateOnNewDocument", json!({"source":"localStorage.setItem('aggr:feed-page-size','10')"})).await?;
        for width in [390, 320] {
            emulate(&client, "Emulation.setDeviceMetricsOverride", json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":true})).await?;
            client.goto(&format!("{}?q=source:example",fixture.base)).await?;
            wait_for(&client,"!!document.querySelector('[data-search-results] .pager button')").await?;
            let state=client.execute(r#"
              const pager=document.querySelector('[data-search-results] .pager');
              const [previous,count,next]=[...pager.children].map(node=>node.getBoundingClientRect());
              const style=getComputedStyle(pager.querySelector('button'));
              return {inline:Math.abs(previous.top-next.top)<1&&count.left>=previous.right&&count.right<=next.left,
                compact:previous.width<100&&next.width<100, touch:previous.height>=44&&next.height>=44,
                plain:style.borderTopWidth==='0px'&&style.backgroundColor==='rgba(0, 0, 0, 0)'};
            "#,vec![]).await?;
            anyhow::ensure!(state==json!({"inline":true,"compact":true,"touch":true,"plain":true}),"pagination stays compact and tappable at {width}px: {state}");
            client.execute("document.querySelector('[data-search-results] .pager').scrollIntoView({block:'center'})",vec![]).await?;
            screenshot(&client,&format!("mobile-search-pagination-{width}")).await?;
        }
        Ok(())
    }.await;
    client.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn search_articles_only_appear_as_results() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        client.goto(&fixture.base).await?;
        wait_for(&client,"!!document.querySelector('.search-command')").await?;
        let nav=client.execute("const nav=document.querySelector('.nav-primary');return {labels:[...nav.querySelectorAll('a')].map(a=>a.textContent.trim()),pipes:nav.querySelectorAll('.nav-separator').length,feed:nav.querySelector('a').href}",vec![]).await?;
        anyhow::ensure!(nav["labels"]==json!(["feed","browse","preferences"]) && nav["pipes"]==2 && nav["feed"]==fixture.base,"desktop menu exposes feed | browse | preferences: {nav}");
        client.find(Locator::Css("#q")).await?.send_keys("Article").await?;
        wait_for(&client,"document.querySelectorAll('.search-results .row').length>1").await?;
        anyhow::ensure!(client.execute("return document.querySelectorAll('.search-completion').length",vec![]).await?==0,"matching article titles must remain in results, never autocomplete");
        // Without suggestions the arrows move the result cursor while typing continues in the field.
        key(&client,"ArrowDown").await?;
        let cursor=client.execute("const rows=[...document.querySelectorAll('.search-results .row')];return {selected:rows.findIndex(row=>row.classList.contains('is-selected')),focused:document.activeElement.id,url:rows[1]?.querySelector('[data-row-open]')?.href}",vec![]).await?;
        anyhow::ensure!(cursor["selected"]==1 && cursor["focused"]=="q","ArrowDown selects the second result without leaving the search field: {cursor}");
        key(&client,"ArrowUp").await?;
        anyhow::ensure!(client.execute("return [...document.querySelectorAll('.search-results .row')].findIndex(row=>row.classList.contains('is-selected'))",vec![]).await?==0,"ArrowUp moves the cursor back to the first result");
        key(&client,"ArrowDown").await?;
        key(&client,"Enter").await?;
        wait_for(&client,"document.body.dataset.kind==='item'").await?;
        anyhow::ensure!(client.execute("return location.href",vec![]).await?==cursor["url"],"Enter opens the selected result when no suggestion is offered");
        client.back().await?;
        wait_for(&client,"document.body.dataset.kind==='river' && !!document.querySelector('#q') && new URL(location.href).searchParams.get('q')==='Article'").await?;
        client.execute("const q=document.querySelector('#q');q.value='source:';q.dispatchEvent(new Event('input',{bubbles:true}))",vec![]).await?;
        wait_for(&client,"document.querySelector('.search-completion')?.dataset.completionId.startsWith('source:')").await?;
        client.find(Locator::Css(".nav-primary [data-feed-action]")).await?.click().await?;
        wait_for(&client,"!new URL(location.href).searchParams.has('q') && !document.querySelector('[data-static-feed]').hidden").await?;
        client.find(Locator::Css(".brand")).await?.click().await?;
        anyhow::ensure!(client.execute("return document.activeElement.id!=='q' && scrollY===0 && !document.querySelector('.search-completions')",vec![]).await?==true,"site title returns to the feed top without focusing search");
        Ok(())
    }.await;
    client.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn search_preview_errors_keep_geometry_and_readable_fallbacks() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        client.goto(&format!("{}?q=source:example", fixture.base)).await?;
        wait_for(&client, "!!document.querySelector('.search-results .preview-image')?.naturalWidth").await?;
        let before = client.execute("const image=document.querySelector('.search-results .preview-image');window.previewImage=image;window.previewSource=image.src;const r=image.parentElement.getBoundingClientRect();image.src=new URL('missing-preview.png',document.baseURI).href;return {width:r.width,height:r.height}", vec![]).await?;
        wait_for(&client,"window.previewImage.parentElement.classList.contains('is-error')").await?;
        let failed=client.execute("const image=window.previewImage,r=image.parentElement.getBoundingClientRect();return {width:r.width,height:r.height,color:getComputedStyle(image).color,alt:image.alt}",vec![]).await?;
        anyhow::ensure!(before["width"]==failed["width"] && before["height"]==failed["height"] && failed["color"]!="rgba(0, 0, 0, 0)" && failed["alt"].as_str().is_some_and(|alt|!alt.is_empty()),"search image failures preserve space and readable alt text: {failed}");
        client.execute("window.previewImage.src=window.previewSource",vec![]).await?;
        wait_for(&client,"window.previewImage.naturalWidth>0 && window.previewImage.parentElement.classList.contains('is-loaded') && !window.previewImage.parentElement.classList.contains('is-error')").await?;
        Ok(())
    }.await;
    client.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn search_control_mount_and_delayed_facets() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let metrics = r#"
      const input=document.querySelector('#q'),r=input.getBoundingClientRect(),s=getComputedStyle(input);
      return {placeholder:input.placeholder,placeholderColor:getComputedStyle(input,'::placeholder').color,placeholderOpacity:getComputedStyle(input,'::placeholder').opacity,width:r.width,height:r.height,top:r.top,left:r.left,background:s.backgroundColor,padding:s.padding,border:s.borderRadius,font:s.font,feedTop:document.querySelector('[data-static-feed]').getBoundingClientRect().top,icons:document.querySelectorAll('[data-search-root] svg').length};
    "#;
    let styles_loaded = "document.querySelectorAll('link[rel=stylesheet]').length > 0 && [...document.querySelectorAll('link[rel=stylesheet]')].every(link => link.sheet)";
    let result = async {
        for width in [1280,390] {
            emulate(&client,"Emulation.setDeviceMetricsOverride",json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":width<600})).await?;
            fixture.scripts_blocked.store(true,Ordering::Relaxed);
            client.goto(&fixture.base).await?;
            wait_for(&client, styles_loaded).await?;
            anyhow::ensure!(client.execute("return typeof window.Swup==='undefined' && !document.querySelector('.search-command')",vec![]).await?==true,"scripts stay blocked while sampling the styled static field");
            let before=client.execute(metrics,vec![]).await?;
            fixture.scripts_blocked.store(false,Ordering::Relaxed);
            client.goto(&fixture.base).await?;
            wait_for(&client,"document.querySelector('.search-input-line')").await?;
            wait_for(&client, styles_loaded).await?;
            let after=client.execute(metrics,vec![]).await?;
            anyhow::ensure!(before==after,"mounting search must preserve its grey field and surrounding geometry at {width}px: before={before}, after={after}");
            anyhow::ensure!(after["icons"]==0 && after["background"]!="rgba(0, 0, 0, 0)","search needs a filled input without a magnifier: {after}");
            client.find(Locator::Css("#q")).await?.send_keys("reading").await?;
            wait_for(&client,"document.querySelector('.search-clear')").await?;
            anyhow::ensure!(client.execute("return getComputedStyle(document.querySelector('#q')).boxShadow",vec![]).await?=="none","search focus must not have a heavy bottom shadow");
            screenshot(&client,&format!("search-clear-focused-{width}")).await?;
            let clear=client.execute(r#"
              const q=document.querySelector('#q').getBoundingClientRect(),button=document.querySelector('.search-clear'),r=button.getBoundingClientRect();
              return {inside:r.left>=q.left&&r.right<=q.right+1&&r.top>=q.top&&r.bottom<=q.bottom+1,large:r.width>=44&&r.height>=44,text:button.textContent.trim(),label:button.getAttribute('aria-label')};
            "#,vec![]).await?;
            anyhow::ensure!(clear["inside"]==true && clear["large"]==true && clear["text"]=="×" && clear["label"]=="Clear search","clear must be an accessible inline cross with a touch target: {clear}");
        }
        emulate(&client,"Page.addScriptToEvaluateOnNewDocument",json!({"source":r#"
          const originalFetch=window.fetch;
          window.fetch=async function(input,...rest){
            if(String(input?.url||input).includes('search-catalog.json')) await new Promise(resolve=>{window.releaseSearchManifest=resolve});
            return originalFetch.call(this,input,...rest);
          };
        "#})).await?;
        client.goto(&fixture.base).await?;
        wait_for(&client,"document.querySelector('.search-input-line')").await?;
        client.find(Locator::Css("#q")).await?.send_keys("source:").await?;
        wait_for(&client,"typeof window.releaseSearchManifest==='function'").await?;
        anyhow::ensure!(client.execute("return document.querySelector('[data-static-feed]').hidden && !document.querySelector('.search-results .row')",vec![]).await?==true,"feed rows stay hidden while the search catalogue loads");
        client.execute("window.releaseSearchManifest()",vec![]).await?;
        wait_for(&client,"[...document.querySelectorAll('.search-completion')].some(row=>row.textContent.toLowerCase().includes('example'))").await?;
        key(&client,"\u{e015}").await?;
        key(&client,"Tab").await?;
        wait_for(&client,"document.querySelector('#q').value.trim()==='source:example' && !document.querySelector('.search-completions')").await?;
        wait_for(&client,"document.querySelector('.search-results .row')").await?;
        Ok(())
    }.await;
    fixture.scripts_blocked.store(false, Ordering::Relaxed);
    client.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn search_focus_scrolls_only_when_obscured() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let config = fixture._directory.path().join("aggr.toml");
    std::fs::write(
        &config,
        std::fs::read_to_string(&config)?.replace("items_per_page=3", "items_per_page=45"),
    )?;
    fixture.build()?;
    let client = browser_client().await?;
    let result = async {
        for width in [1280, 390] {
            emulate(&client, "Emulation.setDeviceMetricsOverride", json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":width<600})).await?;
            client.goto(&fixture.base).await?;
            wait_for(&client, "!!document.querySelector('.search-command') && typeof window.swup?.navigate === 'function'").await?;
            let visible_y = client.execute("const q=document.querySelector('#q').getBoundingClientRect(), header=document.querySelector('.top').getBoundingClientRect();return Math.max(1,Math.floor((q.top-header.bottom)/2))", vec![]).await?;
            for modifier in ["metaKey", "ctrlKey"] {
                client.execute("document.querySelector('#q').blur();window.scrollTo({top:arguments[0],behavior:'instant'})", vec![visible_y.clone()]).await?;
                client.find(Locator::Css("#q")).await?.click().await?;
                anyhow::ensure!(client.execute("return Math.abs(scrollY-arguments[0])<=1 && document.activeElement.id==='q'",vec![visible_y.clone()]).await?==true,"clicking a fully visible search input preserves scroll at {width}px");
                client.execute(&format!("document.dispatchEvent(new KeyboardEvent('keydown',{{key:'k',{modifier}:true,bubbles:true,cancelable:true}}))"),vec![]).await?;
                anyhow::ensure!(client.execute("return Math.abs(scrollY-arguments[0])<=1",vec![visible_y.clone()]).await?==true,"{modifier} preserves scroll when already focused and visible at {width}px");
                client.execute("window.scrollTo({top:650,behavior:'instant'})",vec![]).await?;
                wait_for(&client,"scrollY>500").await?;
                client.execute(&format!("document.dispatchEvent(new KeyboardEvent('keydown',{{key:'k',{modifier}:true,bubbles:true,cancelable:true}}))"),vec![]).await?;
                wait_for(&client,"scrollY===0 && document.activeElement.id==='q'").await?;
            }
            client.execute("document.querySelector('#q').blur();const q=document.querySelector('#q').getBoundingClientRect(),header=document.querySelector('.top').getBoundingClientRect();window.scrollTo({top:q.top-header.bottom+q.height/2,behavior:'instant'})",vec![]).await?;
            let point=client.execute("const q=document.querySelector('#q').getBoundingClientRect();return {x:q.left+20,y:q.bottom-4}",vec![]).await?;
            emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mousePressed","x":point["x"],"y":point["y"],"button":"left","clickCount":1})).await?;
            emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mouseReleased","x":point["x"],"y":point["y"],"button":"left","clickCount":1})).await?;
            wait_for(&client,"scrollY===0 && document.activeElement.id==='q'").await?;
            key(&client,"Escape").await?;
            anyhow::ensure!(client.execute("return document.activeElement.id!=='q' && !document.querySelector('.search-completions')",vec![]).await?==true,"Escape still blurs and dismisses search");
        }
        Ok(())
    }.await;
    if result.is_err() {
        eprintln!("search focus state: {}", client.execute("return {scroll:scrollY,height:document.documentElement.scrollHeight,input:document.querySelector('#q').getBoundingClientRect().toJSON(),header:document.querySelector('.top').getBoundingClientRect().toJSON(),active:document.activeElement.id}",vec![]).await?);
        let _ = screenshot(&client, "search-focus-visibility-failure").await;
    }
    client.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn shift_modifier_highlights_hovered_links() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        client.goto(&fixture.base).await?;
        wait_for(&client,"document.querySelector('.nav .menu-link') && typeof window.swup?.navigate==='function'").await?;
        let point=client.execute("const r=document.querySelector('.nav .menu-link').getBoundingClientRect();return {x:r.left+20,y:r.top+r.height/2}",vec![]).await?;
        emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mouseMoved","x":point["x"],"y":point["y"]})).await?;
        let decoration="getComputedStyle(document.querySelector('.nav .menu-link')).textDecorationLine";
        let before=client.execute(&format!("return {{decoration:{decoration},classes:document.documentElement.className,hovered:[...document.querySelectorAll('a:hover')].map(link=>link.className),selected:document.querySelector('.nav .menu-link').outerHTML}}"),vec![]).await?;
        anyhow::ensure!(before["decoration"]=="none","inactive navigation link normally has no hover underline: {before}");
        for reset in ["release","blur"] {
            emulate(&client,"Input.dispatchKeyEvent",json!({"type":"keyDown","key":"Shift","code":"ShiftLeft","windowsVirtualKeyCode":16,"modifiers":8})).await?;
            wait_for(&client,&format!("{decoration}==='underline'")).await?;
            if reset=="blur" {
                client.execute("window.dispatchEvent(new Event('blur'))",vec![]).await?;
                wait_for(&client,&format!("{decoration}==='none' && !document.documentElement.classList.contains('is-shift-held')")).await?;
            }
            emulate(&client,"Input.dispatchKeyEvent",json!({"type":"keyUp","key":"Shift","code":"ShiftLeft","windowsVirtualKeyCode":16,"modifiers":0})).await?;
            wait_for(&client,&format!("{decoration}==='none' && !document.documentElement.classList.contains('is-shift-held')")).await?;
        }
        anyhow::ensure!(client.current_url().await?.as_str()==fixture.base,"modifier feedback must not navigate");
        client.goto(&format!("{}categories/",fixture.base)).await?;
        for held in [true,false] {
            emulate(&client,"Input.dispatchKeyEvent",json!({"type":if held {"keyDown"} else {"keyUp"},"key":"Shift","code":"ShiftLeft","windowsVirtualKeyCode":16,"modifiers":if held {8} else {0}})).await?;
            anyhow::ensure!(client.execute("return getComputedStyle(document.querySelector('.nav a[aria-current]')).textDecorationLine",vec![]).await?=="underline","selected navigation underline persists regardless of Shift");
        }
        Ok(())
    }.await;
    client.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn media_loading_keeps_reserved_space() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let fixture_image = std::fs::read(fixture.out.join("body.png"))?;
    let archive = fixture._directory.path().join(".aggr/data");
    for index in [36, 37] {
        let path = archive.join(format!("items/example/2026/09/2026-09-01-story-{index}.md"));
        let text = std::fs::read_to_string(&path)?;
        let text = if index == 37 {
            text.replace(
                "https://publisher.invalid/story-37",
                &format!("{}media.webm", fixture.base),
            )
        } else {
            text.replace(
                "content: feed\n",
                &format!(
                    "content: feed\nextra:\n  audio_url: {}media.wav\n",
                    fixture.base
                ),
            )
        };
        std::fs::write(path, text)?;
    }
    let items = archive.join("items/example/2026/09");
    let companions = std::fs::read_dir(&items)?.collect::<std::io::Result<Vec<_>>>()?;
    let preview = companions
        .iter()
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("2026-09-01-story-45.preview-")
        })
        .context("fixture preview")?;
    let original = companions
        .iter()
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("2026-09-01-story-45.image-")
        })
        .context("fixture archived image")?;
    for index in [35, 41] {
        let preview_name = preview
            .file_name()
            .to_string_lossy()
            .replace("story-45", &format!("story-{index}"));
        std::fs::copy(preview.path(), items.join(&preview_name))?;
        let mut metadata = format!(
            "preview:\n  file: {preview_name}\n  width: 240\n  height: 160\n  color: '#315d76'\n"
        );
        if index == 35 {
            let image_name = original
                .file_name()
                .to_string_lossy()
                .replace("story-45", "story-35");
            std::fs::copy(original.path(), items.join(&image_name))?;
            metadata.push_str(&format!("images:\n  - source: {}lead.png\n    original:\n      file: {image_name}\n      width: 640\n      height: 320\n    color: '#315d76'\n",fixture.base));
        }
        let path = items.join(format!("2026-09-01-story-{index}.md"));
        std::fs::write(
            &path,
            std::fs::read_to_string(&path)?
                .replace("content: feed\n", &format!("content: feed\n{metadata}")),
        )?;
    }
    git(&archive, &["add", "items"])?;
    git(&archive, &["commit", "-qm", "fixture native media"])?;
    fixture.build()?;
    std::fs::write(fixture.out.join("body.png"), fixture_image)?;
    std::fs::write(fixture.out.join("document.pdf"), fixture_pdf())?;
    let client = browser_client_with_load_strategy("none").await?;
    let result = media_layout_contracts(&client, &fixture).await;
    fixture.media.blocked.store(false, Ordering::Relaxed);
    fixture.media.app_blocked.store(false, Ordering::Relaxed);
    if let Err(error) = &result {
        let _ = screenshot(&client, "media-layout-failure").await;
        eprintln!("{error:#}");
    }
    client.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn title_metadata_matches_feed_search_and_article() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    emulate(
        &client,
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":"Date.now=()=>Date.parse('2026-09-03T01:00:00Z');"}),
    )
    .await?;
    client.goto(&fixture.base).await?;
    let result = async {
        for width in [1280, 390] {
            emulate(
                &client,
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":width<600}),
            )
            .await?;
            emulate(
                &client,
                "Emulation.setTouchEmulationEnabled",
                json!({"enabled":width<600,"maxTouchPoints":1}),
            )
            .await?;
            for format in ["relative", "iso"] {
                client
                    .execute(
                        "localStorage.setItem('aggr:date-format',arguments[0])",
                        vec![json!(format)],
                    )
                    .await?;
                metadata_contracts(&client, &fixture)
                    .await
                    .with_context(|| format!("metadata at {width}px with {format} dates"))?;
                let expected=if format=="relative" {"4h ago"} else {"2026-09-02"};
                anyhow::ensure!(client.execute("return document.querySelector('.itemhead .dt-published').textContent.trim()",vec![]).await?==expected,"the {format} preference must be applied before comparing metadata");
            }
        }
        Ok(())
    }
    .await;
    client.close().await?;
    result
}

async fn visible_metadata(client: &Client, selector: &str) -> Result<Value> {
    Ok(client.execute(r#"
      return [...document.querySelector(arguments[0]).children]
        .map(field=>{
          const copy=field.cloneNode(true);copy.querySelectorAll('.sr-only').forEach(node=>node.remove());
          const time=field.querySelector('time')?.dateTime || null;
          return {text:copy.textContent.replace(/\s+/g,' ').trim(),links:[...field.querySelectorAll('a')].map(link=>link.href),date:time && !time.startsWith('P') ? new Date(time).toISOString() : time};
        });
    "#,vec![json!(selector)]).await?)
}

async fn metadata_layout(client: &Client, selector: &str) -> Result<Value> {
    Ok(client.execute(r#"
      const meta=document.querySelector(arguments[0]);
      const fields=[...meta.children];
      const style=node=>{const css=getComputedStyle(node);return {
        font:css.fontFamily,size:css.fontSize,weight:css.fontWeight,line:css.lineHeight,
        spacing:css.letterSpacing,color:css.color,align:css.alignItems
      };};
      const textBox=node=>{const range=document.createRange();range.selectNodeContents(node);return range.getBoundingClientRect();};
      const date=meta.querySelector('.published-date'), dateText=textBox(date.querySelector('time'));
      const next=meta.querySelector('.reading-stats'), nextText=textBox(next);
      const source=meta.querySelector('.domain'), resolved=source?.querySelector('.source-resolved'), via=source?.querySelector('em');
      const probe=document.createElement('span');document.body.append(probe);
      probe.style.color='var(--warm)';const warm=getComputedStyle(probe).color;
      probe.style.color='var(--muted)';const muted=getComputedStyle(probe).color;probe.remove();
      return {
        source:{resolved:resolved ? getComputedStyle(resolved).color:null,via:via ? getComputedStyle(via).color:null,warm,muted},
        typography:style(meta),
        fields:fields.map((field,index)=>{
          const css=getComputedStyle(field),separator=getComputedStyle(field,'::before');
          return {typography:style(field),display:css.display,height:field.getBoundingClientRect().height,
            separator:index ? {content:separator.content,left:separator.marginLeft,right:separator.marginRight}:null};
        }),
        date:{text:date.textContent.trim(),trailingSpace:date.getBoundingClientRect().right-dateText.right,
          gap:Math.abs(dateText.top-nextText.top)<2 ? nextText.left-dateText.right:null,
          fontSize:parseFloat(getComputedStyle(date).fontSize)}
      };
    "#,vec![json!(selector)]).await?)
}

async fn metadata_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    client.goto(&fixture.base).await?;
    wait_for(client, "typeof window.swup?.navigate === 'function'").await?;
    let feed = visible_metadata(client, ".row .meta").await?;
    let feed_layout = metadata_layout(client, ".row .meta").await?;
    anyhow::ensure!(
        client.execute("return !!document.querySelector('.row .domain .source-resolved') && !!document.querySelector('.row .reading-stats') && !document.querySelector('.row .tag')",vec![]).await? == true,
        "feed metadata should show its source and reading time, with tags reserved for the article"
    );
    client
        .goto(&format!("{}?q=A%20long%20article%20title", fixture.base))
        .await?;
    wait_for(
        client,
        "document.querySelector('#list .row [data-row-open]')?.href.includes('story-45/')",
    )
    .await?;
    let search = visible_metadata(client, "#list .row .meta").await?;
    let search_layout = metadata_layout(client, "#list .row .meta").await?;
    anyhow::ensure!(
        client
            .execute("return !document.querySelector('#list .row .tag')", vec![])
            .await?
            == true,
        "search metadata must keep tags on the article"
    );
    anyhow::ensure!(
        feed == search,
        "feed/search metadata differ: feed={feed}, search={search}"
    );
    client
        .goto(&format!(
            "{}items/example/2026-09-01-story-45/",
            fixture.base
        ))
        .await?;
    wait_for(client,"!!document.querySelector('.itemhead .meta') && typeof window.swup?.navigate === 'function'").await?;
    let article = visible_metadata(client, ".itemhead .meta").await?;
    let article_layout = metadata_layout(client, ".itemhead .meta").await?;
    anyhow::ensure!(
        client
            .execute("return !!document.querySelector('.item-tags .tag')", vec![])
            .await?
            == true,
        "article tags must remain visible below its metadata"
    );
    let tags = client.execute("const tag=getComputedStyle(document.querySelector('.item-tags .tag'));const meta=getComputedStyle(document.querySelector('.itemhead .meta'));return {color:tag.color,metadataColor:meta.color,font:tag.fontSize,metadataFont:meta.fontSize,background:tag.backgroundColor,border:tag.borderTopWidth,weight:tag.fontWeight}", vec![]).await?;
    anyhow::ensure!(
        tags["color"] == tags["metadataColor"]
            && tags["font"] == tags["metadataFont"]
            && tags["background"] == "rgba(0, 0, 0, 0)"
            && tags["border"] == "0px"
            && tags["weight"] == "400",
        "tags must share the plain muted metadata style: {tags}"
    );
    anyhow::ensure!(
        feed_layout["typography"] == article_layout["typography"]
            && feed_layout["fields"] == article_layout["fields"],
        "shared metadata geometry and typography differ: feed={feed_layout}, article={article_layout}"
    );
    anyhow::ensure!(
        feed_layout["typography"] == search_layout["typography"]
            && feed_layout["fields"] == search_layout["fields"],
        "shared metadata geometry and typography differ: feed={feed_layout}, search={search_layout}"
    );
    for (kind, layout) in [
        ("feed", &feed_layout),
        ("search", &search_layout),
        ("article", &article_layout),
    ] {
        anyhow::ensure!(
            layout["source"]["resolved"] == layout["source"]["warm"]
                && (layout["source"]["via"].is_null()
                    || layout["source"]["via"] == layout["source"]["muted"]),
            "{kind} source should be orange while via text remains muted: {layout}"
        );
        anyhow::ensure!(
            layout["date"]["trailingSpace"]
                .as_f64()
                .is_some_and(|space| space.abs() <= 1.0),
            "{kind} date reserves visible unused spacing: {layout}"
        );
        if let Some(gap) = layout["date"]["gap"].as_f64() {
            anyhow::ensure!(
                (0.0..=layout["date"]["fontSize"].as_f64().unwrap_or_default() * 1.5)
                    .contains(&gap),
                "{kind} date-to-reading-time gap is excessive: {layout}"
            );
        }
    }
    anyhow::ensure!(
        feed == article,
        "feed/article metadata differ: feed={feed}, article={article}"
    );
    Ok(())
}

async fn media_box(client: &Client, selector: &str) -> Result<Value> {
    Ok(client
        .execute(
            r#"
      const media=document.querySelector(arguments[0]), box=media.getBoundingClientRect();
      const following=document.querySelector('[data-media-following]') || media.nextElementSibling || media.parentElement.nextElementSibling;
      return {width:box.width,height:box.height,top:box.top+scrollY,
        following:following ? following.getBoundingClientRect().top+scrollY : null,scroll:scrollY};
    "#,
            vec![json!(selector)],
        )
        .await?)
}

fn ensure_media_box_stable(before: &Value, after: &Value, label: &str) -> Result<()> {
    for field in ["width", "height", "top", "following", "scroll"] {
        if let (Some(before_value), Some(after_value)) =
            (before[field].as_f64(), after[field].as_f64())
        {
            anyhow::ensure!(
                (after_value - before_value).abs() <= 0.05,
                "{label} changed {field}: before={before}, after={after}"
            );
        }
    }
    Ok(())
}

async fn media_layout_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    emulate(client, "Network.enable", json!({})).await?;
    emulate(
        client,
        "Network.setCacheDisabled",
        json!({"cacheDisabled":true}),
    )
    .await?;
    emulate(
        client,
        "Network.setBlockedURLs",
        json!({"urls":["*://player.twitch.tv/*","*://player.vimeo.com/*","*://www.youtube-nocookie.com/*"]}),
    )
    .await?;
    for (index, markup) in [
        (
            40,
            format!(
                "<video id=\"fixture-native\" controls preload=\"metadata\" src=\"{}media.webm\"></video><p data-media-following>After the video.</p>",
                fixture.base
            ),
        ),
        (
            39,
            format!(
                "<audio id=\"fixture-native\" controls preload=\"metadata\" src=\"{}media.wav\"></audio><p data-media-following>After the audio.</p>",
                fixture.base
            ),
        ),
        (
            38,
            format!(
                "<img id=\"fixture-image\" src=\"{}body.png\" alt=\"A fallback illustration\"><p data-media-following>After the image.</p>",
                fixture.base
            ),
        ),
    ] {
        let file = fixture
            .out
            .join(format!("items/example/2026-09-01-story-{index}/index.html"));
        let html = std::fs::read_to_string(&file)?;
        std::fs::write(
            file,
            html.replace(
                "<div class=\"body e-content\">",
                &format!("<div class=\"body e-content\">{markup}"),
            ),
        )?;
    }
    let video = client.execute_async(r#"
      const done=arguments[arguments.length-1];
      const canvas=document.createElement('canvas');canvas.width=640;canvas.height=360;
      const context=canvas.getContext('2d');context.fillStyle='#315d76';context.fillRect(0,0,640,360);
      const stream=canvas.captureStream(10), recorder=new MediaRecorder(stream,{mimeType:'video/webm;codecs=vp8'}), chunks=[];
      recorder.ondataavailable=event=>chunks.push(event.data);
      recorder.onstop=()=>new Blob(chunks).arrayBuffer().then(buffer=>{
        stream.getTracks().forEach(track=>track.stop());
        done(Array.from(new Uint8Array(buffer)));
      });
      recorder.start();setTimeout(()=>recorder.stop(),250);
    "#, vec![]).await?;
    let video: Vec<u8> = serde_json::from_value(video)?;
    std::fs::write(fixture.out.join("media.webm"), video)?;
    let samples = 4000_u32;
    let mut wave = b"RIFF".to_vec();
    wave.extend((samples + 36).to_le_bytes());
    wave.extend(b"WAVEfmt ");
    wave.extend(16_u32.to_le_bytes());
    wave.extend(1_u16.to_le_bytes());
    wave.extend(1_u16.to_le_bytes());
    wave.extend(8000_u32.to_le_bytes());
    wave.extend(8000_u32.to_le_bytes());
    wave.extend(1_u16.to_le_bytes());
    wave.extend(8_u16.to_le_bytes());
    wave.extend(b"data");
    wave.extend(samples.to_le_bytes());
    wave.resize(wave.len() + samples as usize, 128);
    std::fs::write(fixture.out.join("media.wav"), wave)?;
    std::fs::write(
        fixture.out.join("media-player.html"),
        "<!doctype html><html><body style=\"background:#315d76;color:white\">Local video player</body></html>",
    )?;
    for width in [390, 650, 1280] {
        emulate(
            client,
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":width<600}),
        )
        .await?;
        for (index, selector, label, loaded) in [
            (41, ".document-reader", "PDF", "window.documentEvents > 0"),
            (
                35,
                ".article-lead",
                "lead image",
                "document.querySelector('.article-lead img')?.complete",
            ),
            (
                45,
                ".article-picture",
                "archived image",
                "document.querySelector('.progressive-image')?.complete",
            ),
            (
                38,
                "#fixture-image",
                "fallback image",
                "document.querySelector('#fixture-image')?.complete",
            ),
            (
                40,
                "#fixture-native",
                "native video",
                "document.querySelector('#fixture-native')?.readyState >= 1 || document.querySelector('#fixture-native')?.error",
            ),
            (
                39,
                "#fixture-native",
                "native audio",
                "document.querySelector('#fixture-native')?.readyState >= 1 || document.querySelector('#fixture-native')?.error",
            ),
            (
                37,
                ".native-video",
                "direct video",
                "document.querySelector('.native-video video')?.readyState >= 1 || document.querySelector('.native-video video')?.error",
            ),
            (
                36,
                ".native-audio",
                "podcast audio",
                "document.querySelector('.native-audio audio')?.readyState >= 1 || document.querySelector('.native-audio audio')?.error",
            ),
        ] {
            for failed in [false, true] {
                fixture.media.blocked.store(true, Ordering::Relaxed);
                fixture.media.app_blocked.store(true, Ordering::Relaxed);
                fixture.media.failed.store(failed, Ordering::Relaxed);
                client
                    .goto(&format!(
                        "{}items/example/2026-09-01-story-{index}/?media={width}-{failed}",
                        fixture.base
                    ))
                    .await?;
                wait_for(client, &format!("typeof window.swup?.navigate !== 'function' && location.search === '?media={width}-{failed}' && !!document.querySelector('{selector}') && !!document.querySelector('article.item') && getComputedStyle(document.querySelector('.top')).position === 'sticky'")).await?;
                let before_scripts = media_box(client, selector).await?;
                if matches!(label, "PDF" | "lead image" | "archived image") {
                    let placeholder=client.execute("const frame=document.querySelector('.document-frame,.article-lead,.article-picture');const background=getComputedStyle(frame,'::before').backgroundImage;const matched=background.match(/url\\([\"']?(.*?)[\"']?\\)/);window.expectedPlaceholderHref=matched?.[1];return window.expectedPlaceholderHref||null",vec![]).await?;
                    let placeholder = url::Url::parse(
                        placeholder
                            .as_str()
                            .context("computed media placeholder URL")?,
                    )?;
                    anyhow::ensure!(
                        placeholder.as_str().starts_with("data:image/png;base64,"),
                        "{label} must show an inline ThumbHash preview before scripts or image requests complete"
                    );
                    let decoded = client.execute_async("const done=arguments[arguments.length-1],image=new Image();image.onload=()=>done(image.naturalWidth>0 && image.naturalWidth<=32 && image.naturalHeight<=32);image.onerror=()=>done(false);image.src=window.expectedPlaceholderHref",vec![]).await?;
                    anyhow::ensure!(
                        decoded == true,
                        "{label} ThumbHash PNG must decode while media requests remain blocked"
                    );
                }
                client.execute(r#"
                  window.mediaEvents=0;
                  window.documentEvents=0;
                  document.querySelector('.document-viewer')?.addEventListener('load',()=>window.documentEvents++);
                  document.querySelectorAll('embed,iframe,video,audio,img').forEach(media=>{
                    ['load','error','loadedmetadata'].forEach(type=>media.addEventListener(type,()=>window.mediaEvents++));
                  });
                "#, vec![]).await?;
                fixture.media.app_blocked.store(false, Ordering::Relaxed);
                wait_for(client, "typeof window.swup?.navigate === 'function'").await?;
                let mut before_media = media_box(client, selector).await?;
                ensure_media_box_stable(
                    &before_scripts,
                    &before_media,
                    &format!("{label} enhancement at {width}px"),
                )?;
                if label == "PDF" {
                    client.execute_async("const done=arguments[arguments.length-1];scrollTo(0,250);setTimeout(()=>requestAnimationFrame(()=>done(true)),350)",vec![]).await?;
                    before_media = media_box(client, selector).await?;
                }
                if matches!(label, "direct video" | "podcast audio") {
                    client.execute("const media=document.querySelector('.native-video video, .native-audio audio');media.preload='metadata';media.load()",vec![]).await?;
                }
                let completed = fixture.media.completed.load(Ordering::Relaxed);
                fixture.media.blocked.store(false, Ordering::Relaxed);
                let start = Instant::now();
                while fixture.media.completed.load(Ordering::Relaxed) == completed {
                    anyhow::ensure!(
                        start.elapsed() < Duration::from_secs(10),
                        "{label} request did not complete"
                    );
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                wait_for(client, loaded).await?;
                if !failed && label.starts_with("native") {
                    anyhow::ensure!(
                        client
                            .execute(
                                "return document.querySelector('#fixture-native').readyState >= 1",
                                vec![]
                            )
                            .await?
                            == true,
                        "{label} fixture must decode successfully"
                    );
                }
                if !failed && matches!(label, "direct video" | "podcast audio") {
                    anyhow::ensure!(
                        client.execute("return document.querySelector('.native-video video, .native-audio audio').readyState >= 1",vec![]).await?==true,
                        "{label} fixture must decode successfully"
                    );
                }
                if !failed && label.ends_with("image") {
                    anyhow::ensure!(
                        client.execute("return document.querySelector('.article-lead img, #fixture-image, .progressive-image').naturalWidth > 0", vec![]).await? == true,
                        "{label} fixture must decode successfully"
                    );
                }
                client.execute_async("const done=arguments[arguments.length-1];setTimeout(()=>requestAnimationFrame(()=>done(true)),350)", vec![]).await?;
                let after_media = media_box(client, selector).await?;
                ensure_media_box_stable(
                    &before_media,
                    &after_media,
                    &format!(
                        "{label} {} at {width}px",
                        if failed { "failure" } else { "load" }
                    ),
                )?;
                if !failed && label == "direct video" {
                    let timing = client.execute(r#"
                      const media=document.querySelector('.native-video video'),host=document.querySelector('[data-media-timing]');
                      const before=document.querySelector('.body').getBoundingClientRect().top;
                      Object.defineProperties(media,{duration:{configurable:true,value:600},currentTime:{configurable:true,value:60},paused:{configurable:true,value:false}});
                      media.playbackRate=2;
                      media.dispatchEvent(new Event('durationchange'));
                      media.dispatchEvent(new Event('playing'));
                      const playing=host.textContent,metadata=document.querySelector('.itemhead .reading-stats').textContent.trim();
                      Object.defineProperty(media,'paused',{configurable:true,value:true});
                      media.dispatchEvent(new Event('pause'));
                      return {playing,paused:host.textContent,metadata,stable:Math.abs(document.querySelector('.body').getBoundingClientRect().top-before)<=1};
                    "#,vec![]).await?;
                    anyhow::ensure!(
                        timing["playing"]
                            .as_str()
                            .is_some_and(|text| text.contains("Ends at"))
                            && !timing["paused"]
                                .as_str()
                                .unwrap_or_default()
                                .contains("Ends at")
                            && timing["metadata"] == "10 min watch"
                            && timing["stable"] == true,
                        "native video timing is accurate and does not move content: {timing}"
                    );
                }
                fixture.media.failed.store(false, Ordering::Relaxed);
            }
        }
        for (index, provider) in [(43, "twitch"), (42, "vimeo")] {
            for motion in ["auto", "off"] {
                client
                    .execute(
                        "localStorage.setItem('aggr:motion',arguments[0])",
                        vec![json!(motion)],
                    )
                    .await?;
                client
                    .goto(&format!(
                        "{}items/example/2026-09-01-story-{index}/?provider={width}-{motion}",
                        fixture.base
                    ))
                    .await?;
                wait_for(
                client,
                &format!("location.search === '?provider={width}-{motion}' && document.querySelector('.video-player')?.dataset.videoProvider === '{provider}' && document.querySelector('[data-video-embed]')?.getAttribute('role') === 'button'"),
            )
            .await?;
                client
                    .execute(
                        r#"
              document.querySelector('[data-video-embed]').dataset.videoEmbed=arguments[0];
            "#,
                        vec![json!(format!(
                            "{}media-player.html?provider={provider}&width={width}&motion={motion}",
                            fixture.base
                        ))],
                    )
                    .await?;
                let before = media_box(client, ".video-player").await?;
                fixture.media.blocked.store(true, Ordering::Relaxed);
                client
                    .execute(
                        "document.querySelector('[data-video-embed]').click()",
                        vec![],
                    )
                    .await?;
                wait_for(client, "!!document.querySelector('.video-player iframe')").await?;
                client.execute_async("const done=arguments[arguments.length-1];requestAnimationFrame(()=>requestAnimationFrame(()=>done(true)))", vec![]).await?;
                let after = media_box(client, ".video-player").await?;
                ensure_media_box_stable(
                    &before,
                    &after,
                    &format!("{provider} activation at {width}px"),
                )?;
                let pending = client.execute(r#"
              const player=document.querySelector('.video-player'), preview=player.querySelector('.video-preview'), frame=player.querySelector('iframe'), style=getComputedStyle(frame);
              return {preview:!!preview && !preview.hidden && getComputedStyle(preview).display !== 'none',covered:style.opacity === '0' || style.visibility === 'hidden' || style.display === 'none',classes:player.className,source:frame.src,opacity:style.opacity,visibility:style.visibility,display:style.display};
            "#,vec![]).await?;
                anyhow::ensure!(
                    pending["preview"] == true && pending["covered"] == true,
                    "{provider} at {width}px with motion={motion} must keep its preview visible until ready: {pending}"
                );
                fixture.media.blocked.store(false, Ordering::Relaxed);
                wait_for(
                    client,
                    "document.querySelector('.video-player').classList.contains('is-loaded')",
                )
                .await?;
                let loaded = media_box(client, ".video-player").await?;
                ensure_media_box_stable(
                    &before,
                    &loaded,
                    &format!("{provider} ready at {width}px with motion={motion}"),
                )?;
            }
        }
    }
    Ok(())
}

async fn run_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    set_offline(client, false).await?;
    client.set_window_rect(0, 0, 390, 844).await?;
    emulate(
        client,
        "Emulation.setEmulatedMedia",
        json!({"features":[{"name":"prefers-reduced-motion","value":"no-preference"}]}),
    )
    .await?;
    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":390,"height":844,"deviceScaleFactor":1,"mobile":true}),
    )
    .await?;
    emulate(
        client,
        "Emulation.setTouchEmulationEnabled",
        json!({"enabled":true}),
    )
    .await?;
    client.goto(&fixture.base).await?;
    wait_for(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3 && typeof window.swup?.navigate === 'function'",
    )
    .await?;
    assert_eq!(client.execute(r#"
      const cached = window.swup.cache.get(location.href);
      return !!cached && !new DOMParser().parseFromString(cached.html,'text/html').querySelector('#swup [data-bound]');
    "#, vec![]).await?, true, "the initial page must be cached before event-binding markers are added");
    let shared_fetch = client.execute_async(r#"
      const done = arguments[arguments.length-1];
      const target = new URL('browse/?prefetch-contract=1',document.baseURI);
      Promise.all([window.swup.fetchPage(target.href), window.swup.fetchPage(target.pathname+target.search)]).then(pages => {
        done({requests:performance.getEntriesByName(target.href).length, sameHtml:pages[0].html===pages[1].html});
      }).catch(error => done({error:String(error)}));
    "#, vec![]).await?;
    assert_eq!(
        shared_fetch,
        json!({"requests":1,"sameHtml":true}),
        "concurrent prefetch/navigation requests should share one download"
    );
    client.execute(r#"
      const state = {
        originalFetch:window.fetch, home:location.href,
        destination:new URL('browse/?prefetch-cancellation=destination',document.baseURI).href,
        unrelated:new URL('preferences/?prefetch-cancellation=unrelated',document.baseURI).href,
        requests:{}, aborted:[], release:{}, settled:{}
      };
      window.prefetchCancellationContract = state;
      window.fetch = function(input, options) {
        const url = new URL(typeof input === 'string' ? input : input.url, document.baseURI);
        const key = url.searchParams.get('prefetch-cancellation');
        if (!key) return state.originalFetch.call(window,input,options);
        state.requests[key] = (state.requests[key] || 0) + 1;
        return new Promise((resolve,reject) => {
          const signal = options.signal;
          const abort = () => {
            state.aborted.push(key);
            reject(new DOMException('Canceled','AbortError'));
          };
          if (signal.aborted) { abort(); return; }
          signal.addEventListener('abort',abort,{once:true});
          state.release[key] = () => {
            state.originalFetch.call(window,input,options).then(resolve,reject).finally(() => signal.removeEventListener('abort',abort));
          };
        });
      };
      for (const key of ['destination','unrelated']) {
        window.swup.fetchPage(state[key],{priority:'low'}).then(
          () => { state.settled[key] = 'fulfilled'; },
          () => { state.settled[key] = 'rejected'; }
        );
      }
    "#, vec![]).await?;
    wait_for(
        client,
        "Object.keys(window.prefetchCancellationContract.requests).length === 2",
    )
    .await?;
    client
        .execute(
            "window.swup.navigate(window.prefetchCancellationContract.destination)",
            vec![],
        )
        .await?;
    wait_for(
        client,
        "window.prefetchCancellationContract.settled.unrelated === 'rejected'",
    )
    .await?;
    assert_eq!(client.execute(r#"
      const state = window.prefetchCancellationContract;
      return {requests:state.requests,aborted:state.aborted,destinationPending:!state.settled.destination};
    "#, vec![]).await?, json!({
        "requests":{"destination":1,"unrelated":1},
        "aborted":["unrelated"],
        "destinationPending":true
    }), "navigation must cancel unrelated speculation while retaining its shared destination request");
    client
        .execute(
            "window.prefetchCancellationContract.release.destination()",
            vec![],
        )
        .await?;
    wait_for(client, "document.body.dataset.kind === 'browse' && window.prefetchCancellationContract.settled.destination === 'fulfilled'").await?;
    assert_eq!(
        client
            .execute(
                "return window.prefetchCancellationContract.requests.destination",
                vec![]
            )
            .await?,
        1,
        "the destination must not download again when speculation becomes navigation"
    );
    client
        .execute(
            r#"
      const state = window.prefetchCancellationContract;
      window.fetch = state.originalFetch;
      window.swup.cache.delete(state.destination);
      window.swup.cache.delete(state.unrelated);
      window.swup.navigate(state.home);
    "#,
            vec![],
        )
        .await?;
    wait_for(client, "location.href === window.prefetchCancellationContract.home && document.body.dataset.kind === 'river' && document.querySelectorAll('[data-row-open]').length === 3").await?;
    client
        .execute("delete window.prefetchCancellationContract", vec![])
        .await?;
    for (index, provider, host) in [
        (44, "youtube", "www.youtube-nocookie.com"),
        (43, "twitch", "player.twitch.tv"),
        (42, "vimeo", "player.vimeo.com"),
    ] {
        client
            .goto(&format!(
                "{}items/example/2026-09-01-story-{index}/",
                fixture.base
            ))
            .await?;
        if provider == "youtube" {
            wait_for(client, "!!document.querySelector('.video-player iframe')").await?;
            let source_colors = client.execute(r#"
              const source=document.querySelector('.itemhead .domain:has(em)'), span=source.querySelector('.source-resolved');
              const probe=document.createElement('span');probe.style.color='var(--warm)';document.body.append(probe);
              const result={source:getComputedStyle(span).color,warm:getComputedStyle(probe).color,via:getComputedStyle(source.querySelector('em')).color,neutral:getComputedStyle(source).color};probe.remove();return result;
            "#, vec![]).await?;
            assert_eq!(source_colors["source"], source_colors["warm"]);
            assert_eq!(source_colors["via"], source_colors["neutral"]);
            assert_ne!(source_colors["source"], source_colors["via"]);
            assert_eq!(client.execute("const date=document.querySelector('.published-date');date.dispatchEvent(new PointerEvent('pointerover',{bubbles:true}));return {duplicate:date.title.includes('Updated:'),separate:date.hasAttribute('data-date-updated')}", vec![]).await?, json!({"duplicate":false,"separate":false}), "identical published and updated dates should appear only once in the tooltip");
            client.execute(r#"
              const frame=document.querySelector('.video-player iframe');
              window.videoContract={url:frame.src,allow:frame.allow,sandbox:frame.getAttribute('sandbox'),referrer:frame.referrerPolicy,title:frame.title};
              frame.removeAttribute('src');
            "#, vec![]).await?;
        } else {
            wait_for(
                client,
                "document.querySelector('[data-video-embed]')?.getAttribute('role') === 'button'",
            )
            .await?;
            assert_eq!(client.execute("return {frames:document.querySelectorAll('iframe').length,providerRequests:performance.getEntriesByType('resource').filter(r=>/youtube|twitch|vimeo/.test(new URL(r.name).hostname)).length}",vec![]).await?,json!({"frames":0,"providerRequests":0}),"video providers must not receive requests before activation");
            client.execute(r#"
          const player=document.querySelector('.video-player'), append=player.appendChild;
          player.appendChild=function(frame){
            window.videoContract={url:frame.src,allow:frame.allow,sandbox:frame.getAttribute('sandbox'),referrer:frame.referrerPolicy,title:frame.title};
            frame.removeAttribute('src');
            return append.call(this,frame);
          };
        "#,vec![]).await?;
            client
                .find(Locator::Css("[data-video-embed]"))
                .await?
                .click()
                .await?;
        }
        let video = client.execute(r#"
          const frame=document.querySelector('.video-player iframe'), url=new URL(window.videoContract.url);
          return {...window.videoContract,host:url.hostname,parent:url.searchParams.get('parent'),autoplay:url.searchParams.get('autoplay'),rel:url.searchParams.get('rel'),dnt:url.searchParams.get('dnt'),h:url.searchParams.get('h'),frames:document.querySelectorAll('iframe').length,width:frame.clientWidth,height:frame.clientHeight,overflow:document.documentElement.scrollWidth>innerWidth};
        "#,vec![]).await?;
        assert_eq!(video["host"], host, "{video}");
        assert_eq!(video["frames"], 1);
        assert_eq!(video["referrer"], "strict-origin-when-cross-origin");
        assert_eq!(
            video["allow"],
            "autoplay; encrypted-media; fullscreen; picture-in-picture"
        );
        assert_eq!(
            video["sandbox"],
            "allow-scripts allow-same-origin allow-presentation"
        );
        assert!(video["title"].as_str().is_some_and(|s| s.contains("Play")));
        assert_eq!(video["overflow"], false, "{video}");
        if provider == "twitch" {
            assert_eq!(video["parent"], "127.0.0.1");
            assert_eq!(video["autoplay"], "true");
            assert!(
                video["width"].as_f64().unwrap_or_default() >= 400.0
                    && video["height"].as_f64().unwrap_or_default() >= 300.0,
                "{video}"
            );
        } else {
            assert_eq!(
                video["autoplay"],
                if provider == "youtube" { "0" } else { "1" }
            );
            assert_eq!(video["parent"], Value::Null);
        }
        if provider == "youtube" {
            assert_eq!(video["rel"], "0");
        }
        if provider == "vimeo" {
            assert_eq!(video["dnt"], "1");
            assert_eq!(video["h"], "abc123def4");
        }
    }
    client.goto(&fixture.base).await?;
    wait_for(
        client,
        "document.documentElement.classList.contains('swup-enabled')",
    )
    .await?;
    let preference_checks = client
        .execute_async(
            include_str!("fixtures/preferences_checks.js"),
            vec![json!(std::fs::read_to_string(
                fixture.out.join("index.html")
            )?)],
        )
        .await?;
    assert!(
        preference_checks.get("error").is_none(),
        "{preference_checks}"
    );
    assert_eq!(preference_checks["checks"], 10);
    let feed_page_size = client
        .execute(
            r#"
      const list = document.querySelector('.rows');
      const pager = document.querySelector('[data-feed-pager]');
      const original = list.querySelector('.row');
      while (list.querySelectorAll('.row').length < 50) {
        const index = list.querySelectorAll('.row').length;
        const row = original.cloneNode(true);
        row.classList.remove('is-selected');
        row.dataset.virtualIndex = String(index);
        row.querySelector('[data-row-open]').href = new URL('items/example/virtual-' + index + '/', location.href);
        row.querySelector('.rank').textContent = (index + 1) + '.';
        list.appendChild(row);
      }
      Array.from(list.querySelectorAll('.row')).forEach(function (row, index) { row.dataset.virtualIndex = String(index); });
      const firstStatic = location.pathname;
      const secondStatic = new URL('page/2/', location.href).pathname;
      Object.assign(pager.dataset, {
        staticPage:'1', staticPages:'2', staticPageSize:'50', totalItems:'75',
        staticFirst:firstStatic, staticLast:secondStatic, staticNext:secondStatic
      });
      delete pager.dataset.staticPrevious;
      localStorage.setItem('aggr:feed-page-size', '25');
      window.dispatchEvent(new StorageEvent('storage', {key:'aggr:feed-page-size'}));
      const first = Array.from(list.querySelectorAll('.row:not([hidden])'), row => row.dataset.virtualIndex);
      for (let i = 0; i < 30; i += 1) document.dispatchEvent(new KeyboardEvent('keydown', {key:'j', bubbles:true}));
      const cursor = list.querySelector('.row.is-selected');
      const firstState = {
        status:pager.querySelector('[data-page-status]').textContent,
        next:new URL(pager.querySelector('[data-page-next]').href).searchParams.get('feed-page'),
        cursor:cursor && cursor.dataset.virtualIndex,
        cursorHidden:cursor && cursor.hidden
      };
      const secondUrl = new URL(location.href);
      secondUrl.searchParams.set('feed-page', '2');
      history.replaceState(history.state, '', secondUrl);
      window.dispatchEvent(new StorageEvent('storage', {key:'aggr:feed-page-size'}));
      const second = Array.from(list.querySelectorAll('.row:not([hidden])'), row => row.dataset.virtualIndex);
      const secondState = {
        selected:list.querySelector('.row.is-selected')?.dataset.virtualIndex,
        status:pager.querySelector('[data-page-status]').textContent,
        previous:new URL(pager.querySelector('[data-page-previous]').href).searchParams.has('feed-page'),
        nextPath:new URL(pager.querySelector('[data-page-next]').href).pathname,
        nextSlice:new URL(pager.querySelector('[data-page-next]').href).searchParams.has('feed-page'),
        nextHidden:pager.querySelector('[data-page-next]').hidden
      };
      const finalRows = [];
      for (let index = 50; index < 75; index += 1) {
        const row = original.cloneNode(true);
        row.hidden = false;
        row.classList.remove('is-selected');
        row.dataset.virtualIndex = String(index);
        row.querySelector('[data-row-open]').href = new URL('items/example/virtual-' + index + '/', location.href);
        row.querySelector('.rank').textContent = (index + 1) + '.';
        finalRows.push(row);
      }
      list.replaceChildren(...finalRows);
      Object.assign(pager.dataset, {staticPage:'2', staticPrevious:firstStatic});
      delete pager.dataset.staticNext;
      history.replaceState(history.state, '', secondStatic);
      window.dispatchEvent(new StorageEvent('storage', {key:'aggr:feed-page-size'}));
      const final = Array.from(list.querySelectorAll('.row:not([hidden])'), row => row.dataset.virtualIndex);
      const finalState = {
        status:pager.querySelector('[data-page-status]').textContent,
        previousPath:new URL(pager.querySelector('[data-page-previous]').href).pathname,
        previousSlice:new URL(pager.querySelector('[data-page-previous]').href).searchParams.get('feed-page'),
        nextHidden:pager.querySelector('[data-page-next]').hidden
      };
      localStorage.removeItem('aggr:feed-page-size');
      window.dispatchEvent(new StorageEvent('storage', {key:'aggr:feed-page-size'}));
      const defaultVisible = list.querySelectorAll('.row:not([hidden])').length;
      const defaultPagerHidden = pager.hidden;
      history.replaceState(history.state, '', firstStatic);
      return {
        first:first, second:second, final:final,
        all:[...new Set(first.concat(second, final))].length,
        firstState:firstState, secondState:secondState, finalState:finalState,
        defaultVisible:defaultVisible, defaultPagerHidden:defaultPagerHidden
      };
    "#,
            vec![],
        )
        .await?;
    assert_eq!(feed_page_size["first"].as_array().map(Vec::len), Some(25));
    assert_eq!(feed_page_size["second"].as_array().map(Vec::len), Some(25));
    assert_eq!(feed_page_size["final"].as_array().map(Vec::len), Some(25));
    assert_eq!(feed_page_size["all"], 75);
    assert_eq!(
        feed_page_size["firstState"],
        json!({"status":"page 1 / 3","next":"2","cursor":"24","cursorHidden":false})
    );
    assert_eq!(
        feed_page_size["secondState"],
        json!({"selected":"25","status":"page 2 / 3","previous":false,"nextPath":"/reader/page/2/","nextSlice":false,"nextHidden":false})
    );
    assert_eq!(
        feed_page_size["finalState"],
        json!({"status":"page 3 / 3","previousPath":"/reader/","previousSlice":"2","nextHidden":true})
    );
    assert_eq!(feed_page_size["defaultVisible"], 25);
    assert_eq!(feed_page_size["defaultPagerHidden"], false);
    client
        .execute("localStorage.setItem('aggr:theme','light')", vec![])
        .await?;
    client.refresh().await?;
    wait_for(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3 && typeof window.swup?.navigate === 'function'",
    )
    .await?;
    let layout = client.execute(r#"
      const nav = document.querySelector('.nav');
      const config = document.querySelector('.config-link');
      const row = document.querySelectorAll('.row')[1];
      const title = row.querySelector('.title');
      const meta = row.querySelector('.meta > *');
      const brand = document.querySelector('.brand-icon');
      const top = document.querySelector('.top');
      const topStyle = getComputedStyle(top);
      const topNav = document.querySelector('.nav');
      const topNavStyle = getComputedStyle(topNav);
      const brandLinkStyle = getComputedStyle(document.querySelector('.brand'));
      const configStyle = getComputedStyle(config);
      const rank = document.querySelector('.rank');
      return {count:nav.querySelectorAll('a').length, visible:[...nav.querySelectorAll('a')].filter(link => link.getBoundingClientRect().height > 0).length===2 && [...document.querySelectorAll('.mobile-tabs a')].every(link=>link.getBoundingClientRect().height>=44),
        labels:[...nav.querySelectorAll('.menu-link')].map(link=>link.textContent.trim()),
        separators:[...nav.querySelectorAll('.nav-primary .nav-separator')].map(span=>({text:span.textContent,hidden:span.getAttribute('aria-hidden')})),
        searchIcon:!!nav.querySelector('[data-search-open]'),
        searchBelowNav:document.querySelector('[data-search-root]').getBoundingClientRect().top >= top.getBoundingClientRect().bottom,
        searchAboveFeed:document.querySelector('[data-search-root]').getBoundingClientRect().bottom <= document.querySelector('[data-static-feed]').getBoundingClientRect().top,
        flat:!document.querySelector('#site-menu, .feed-tabs'),
        overflow:document.documentElement.scrollWidth > document.documentElement.clientWidth + 1,
        reload:!!document.querySelector('#refresh-page'), density:document.documentElement.dataset.density,
        configVisible:getComputedStyle(config).display !== 'none',
        configRight:innerWidth-config.getBoundingClientRect().right,
        selected:document.querySelectorAll('.row.is-selected').length,
        brandLeft:brand.getBoundingClientRect().left, rankDisplay:getComputedStyle(rank).display,
        rankWidth:rank.getBoundingClientRect().width,
        titleLeft:title.getBoundingClientRect().left,metaLeft:meta.getBoundingClientRect().left,
        metadataInset:parseFloat(getComputedStyle(row.querySelector(".meta")).paddingLeft),
        header:{height:top.getBoundingClientRect().height,position:topStyle.position,
          background:topStyle.backgroundColor,navMinHeight:topNavStyle.minHeight,
          navPaddingLeft:topNavStyle.paddingLeft,navPaddingRight:topNavStyle.paddingRight,
          brandMarginLeft:brandLinkStyle.marginLeft,brandMarginRight:brandLinkStyle.marginRight,
          configFontSize:configStyle.fontSize,configPaddingLeft:configStyle.paddingLeft,
          configPaddingRight:configStyle.paddingRight},
        rowHeight:row.getBoundingClientRect().height,
        rowInset:title.getBoundingClientRect().left-row.getBoundingClientRect().left,
        rowInlinePadding:parseFloat(getComputedStyle(row).paddingLeft),
        rowPadding:parseFloat(getComputedStyle(row).paddingTop)};
    "#, vec![]).await?;
    assert_eq!(layout["count"], 5);
    assert_eq!(layout["labels"], json!(["feed", "browse", "preferences"]));
    assert_eq!(
        layout["separators"],
        json!([{ "text": "|", "hidden": "true" }])
    );
    assert_eq!(layout["searchIcon"], false);
    assert_eq!(layout["flat"], true);
    assert_eq!(layout["searchBelowNav"], true);
    assert_eq!(layout["searchAboveFeed"], true);
    assert_eq!(layout["visible"], true);
    assert_eq!(layout["overflow"], false);
    assert_eq!(layout["reload"], false);
    assert_eq!(layout["density"], "compact");
    assert_eq!(layout["configVisible"], true);
    assert!(
        layout["configRight"].as_f64().unwrap_or_default() <= 17.0,
        "Preferences and config should stay at the mobile header's right edge: {layout}"
    );
    assert_eq!(layout["selected"], 1, "the first feed row starts selected");
    assert_eq!(layout["rankDisplay"], "none", "mobile ranks are hidden");
    assert_eq!(layout["rankWidth"], 0, "hidden ranks reserve no width");
    assert_eq!(
        layout["metadataInset"], 0,
        "metadata reclaims the rank column"
    );
    assert!(
        (layout["metaLeft"].as_f64().unwrap_or_default()
            - layout["titleLeft"].as_f64().unwrap_or_default())
        .abs()
            <= 1.0,
        "mobile metadata should share the title text axis: {layout}"
    );
    let compact_row_height = layout["rowHeight"].as_f64().context("compact row height")?;
    assert!(
        compact_row_height <= 140.0,
        "compact mobile row should stay dense: {layout}"
    );
    assert!(
        (layout["rowInset"].as_f64().unwrap_or_default()
            - layout["rowInlinePadding"].as_f64().unwrap_or_default())
        .abs()
            <= 1.0,
        "mobile titles reclaim the rank column and retain only the row inset: {layout}"
    );
    assert!(
        layout["rowPadding"].as_f64().unwrap_or_default() >= 4.5,
        "compact rows still need breathing room: {layout}"
    );
    key(client, "?").await?;
    wait_for(client, "document.querySelector('#shortcut-help').open").await?;
    let shortcut_dialog = client
        .execute(
            r#"
      const dialog = document.querySelector('#shortcut-help');
      const close = document.querySelector('.shortcut-close');
      const box = dialog.getBoundingClientRect();
      return {x:Math.abs((box.left+box.right)/2-innerWidth/2),
        y:Math.abs((box.top+box.bottom)/2-innerHeight/2),
        focus:document.activeElement?.id,
        closeDecoration:getComputedStyle(close).textDecorationLine,
        overflow:dialog.scrollWidth > dialog.clientWidth};
    "#,
            vec![],
        )
        .await?;
    assert!(
        shortcut_dialog["x"].as_f64().unwrap_or(f64::MAX) <= 1.0
            && shortcut_dialog["y"].as_f64().unwrap_or(f64::MAX) <= 1.0,
        "shortcut help must be centered in both axes: {shortcut_dialog}"
    );
    assert_eq!(shortcut_dialog["focus"], "shortcut-help-title");
    assert_eq!(shortcut_dialog["closeDecoration"], "none");
    assert_eq!(client.execute("return [...document.querySelectorAll('#shortcut-help .key-pair')].map(pair=>pair.textContent).filter(text=>text==='Ctrl+d'||text==='gthenf')", vec![]).await?, json!(["Ctrl+d","gthenf"]), "help must distinguish simultaneous keys from sequences");
    assert_eq!(
        shortcut_dialog["overflow"], false,
        "help should fit a phone: {shortcut_dialog}"
    );
    screenshot(client, "mobile-shortcuts").await?;
    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":320,"height":720,"deviceScaleFactor":1,"mobile":true}),
    )
    .await?;
    assert_eq!(client.execute("const dialog=document.querySelector('#shortcut-help');return dialog.scrollWidth <= dialog.clientWidth;", vec![]).await?, true, "shortcut help must fit a 320px viewport");
    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":390,"height":844,"deviceScaleFactor":1,"mobile":true}),
    )
    .await?;
    key(client, "Escape").await?;
    wait_for(client, "!document.querySelector('#shortcut-help').open").await?;
    wait_for(
        client,
        "document.querySelector('.preview-image')?.naturalWidth === 240",
    )
    .await?;
    assert_eq!(
        client
            .execute("return window.swup.options.native", vec![])
            .await?,
        false,
        "touch navigation must not wait for full-page transition snapshots"
    );
    wait_for(
        client,
        "window.swup.cache.has(document.querySelector('[data-row-open]').href)",
    )
    .await?;
    screenshot(client, "mobile-feed").await?;
    client
        .execute("document.querySelector('.brand').focus()", vec![])
        .await?;
    key(client, "/").await?;
    anyhow::ensure!(
        client
            .execute(
                "return document.activeElement?.classList.contains('brand')",
                vec![]
            )
            .await?
            == true,
        "slash must not open search"
    );
    key(client, "\u{e009}k\u{e000}").await?;
    wait_for(
        client,
        "document.activeElement?.id === 'q' && document.body.dataset.kind === 'river'",
    )
    .await?;
    assert_eq!(
        client.execute("return location.pathname", vec![]).await?,
        "/reader/",
        "the search shortcut focuses the feed input without opening the selected article"
    );
    key(client, "Escape").await?;
    assert_eq!(
        client
            .execute(
                "return document.querySelectorAll('.row .domain').length",
                vec![]
            )
            .await?,
        3,
        "every feed row should show its source below the title"
    );

    for (route, path, title) in [
        ("browse/", "/reader/browse/", "browse | Reading room | aggr"),
        (
            "preferences/",
            "/reader/preferences/",
            "preferences | Reading room | aggr",
        ),
        ("", "/reader/", "Reading room | aggr"),
    ] {
        let selector = if route.is_empty() {
            ".brand".to_string()
        } else {
            format!(".mobile-tabs a[data-route='{route}']")
        };
        client.find(Locator::Css(&selector)).await?.click().await?;
        wait_for(
            client,
            &format!(
                "location.pathname === {path} && document.title === {title}",
                path = serde_json::to_string(path)?,
                title = serde_json::to_string(title)?
            ),
        )
        .await?;
        assert_eq!(
            client
                .execute("return document.querySelector('#aggr-base').href", vec![])
                .await?,
            fixture.base,
            "the persistent document base must keep every mobile route site-relative"
        );
        assert_eq!(client.title().await?, title, "{path} document title");
    }

    key(client, "\u{e03d}k\u{e000}").await?;
    wait_for(
        client,
        "location.pathname === '/reader/' && document.activeElement?.id === 'q'",
    )
    .await?;
    for (directory, kind, value) in [
        ("browse", "", ""),
        ("sources/example", "source", "example"),
        ("tags/reading", "tag", "reading"),
        ("categories/engineering", "category", "engineering"),
    ] {
        let destination = format!("{}{directory}/", fixture.base);
        client.goto(&destination).await?;
        wait_for(
            client,
            "document.documentElement.classList.contains('swup-enabled')",
        )
        .await?;
        let scopes = client.execute("const root=document.querySelector('[data-search-root]');return root ? {kind:root.dataset.scopeKind,value:root.dataset.scopeValue} : null", vec![]).await?;
        if kind.is_empty() {
            assert_eq!(
                scopes,
                Value::Null,
                "directory pages use the global search shortcut"
            );
        } else {
            assert_eq!(scopes, json!({"kind":kind,"value":value}));
        }
        key(client, "\u{e009}k\u{e000}").await?;
        wait_for(client, "document.activeElement?.id === 'q'").await?;
        let path = if kind.is_empty() {
            "/reader/".to_string()
        } else {
            format!("/reader/{directory}/")
        };
        assert_eq!(
            client.current_url().await?.path(),
            path,
            "search should retain the current archive scope and route"
        );
        assert_eq!(
            client
                .execute("return document.querySelectorAll('#q').length", vec![])
                .await?,
            1
        );
    }
    for route in ["", "browse/", "?q=article", "preferences/"] {
        client.goto(&format!("{}{route}", fixture.base)).await?;
        wait_for(
            client,
            "document.documentElement.classList.contains('swup-enabled')",
        )
        .await?;
        let scrolling = client.execute(r#"
          document.activeElement?.blur();
          const original = window.scrollBy, calls = [];
          window.scrollBy = options => calls.push(options.top);
          for (const [key, ctrlKey] of [['d',false],['u',false],['d',true],['u',true],['e',true],['y',true]]) {
            document.body.dispatchEvent(new KeyboardEvent('keydown',{key,ctrlKey,bubbles:true,cancelable:true}));
          }
          const input=document.createElement('input'); document.body.append(input); input.focus();
          for (const key of ['d','u','e','y']) input.dispatchEvent(new KeyboardEvent('keydown',{key,ctrlKey:true,bubbles:true,cancelable:true}));
          input.remove(); window.scrollBy=original;
          return calls;
        "#, vec![]).await?;
        let calls = scrolling.as_array().context("scroll calls")?;
        assert_eq!(
            calls.len(),
            6,
            "all scrolling motions work outside inputs on {route}"
        );
        assert_eq!(calls[0], calls[2]);
        assert_eq!(calls[1], calls[3]);
        for pair in calls.chunks(2) {
            let down = pair[0].as_f64().context("down distance")?;
            assert!(down > 0.0 && down <= 400.0);
            assert_eq!(pair[1].as_f64(), Some(-down));
        }
    }
    for (width, mobile) in [(1280, false), (390, true)] {
        emulate(
            client,
            "Emulation.setTouchEmulationEnabled",
            json!({"enabled":mobile}),
        )
        .await?;
        emulate(
            client,
            "Emulation.setDeviceMetricsOverride",
            json!({
                "width":width,"height":844,"deviceScaleFactor":1,"mobile":mobile
            }),
        )
        .await?;
        client.goto(&fixture.base).await?;
        wait_for(
            client,
            "typeof window.swup?.navigate === 'function' && !!document.querySelector('.row [data-row-open]')",
        )
        .await?;
        assert_eq!(client.execute("return document.querySelector('.row.is-selected') === document.querySelector('.row:not([hidden])') && !document.activeElement.matches('[data-row-open]') && scrollY === 0", vec![]).await?, true,
            "initial selection marks the first visible row without moving focus or scrolling");
        if !mobile {
            for (selector, decoration) in
                [(".row .rank", "underline"), (".row .category a", "none")]
            {
                let point = client.execute("const rect=document.querySelector(arguments[0]).getBoundingClientRect(); return {x:rect.left+rect.width/2,y:rect.top+rect.height/2}", vec![json!(selector)]).await?;
                emulate(
                    client,
                    "Input.dispatchMouseEvent",
                    json!({
                        "type":"mouseMoved","x":point["x"],"y":point["y"]
                    }),
                )
                .await?;
                client.execute_async("const done=arguments[arguments.length-1];requestAnimationFrame(()=>requestAnimationFrame(()=>done(true)))", vec![]).await?;
                assert_eq!(client.execute("return getComputedStyle(document.querySelector('.row .title')).textDecorationLine", vec![]).await?, decoration,
                    "hovering row space highlights its title; hovering metadata highlights only its own link");
            }
        }
        let row_actions = client.execute(r#"
          const row = document.querySelector('.row');
          const title = row.querySelector('[data-row-open]');
          const rank = row.querySelector('.rank');
          const before = {row:row.getBoundingClientRect().left, title:title.getBoundingClientRect().left, rank:rank.getBoundingClientRect().left, background:getComputedStyle(row).backgroundColor};
          const forwarded = [];
          const capture = event => {
            forwarded.push({type:event.type, button:event.button, ctrl:event.ctrlKey, shift:event.shiftKey});
            event.preventDefault();
            event.stopPropagation();
          };
          title.addEventListener('click', capture);
          title.addEventListener('auxclick', capture);
          const surface=getComputedStyle(rank).display==='none' ? row : rank;
          surface.dispatchEvent(new MouseEvent('click', {bubbles:true,cancelable:true,ctrlKey:true,shiftKey:true}));
          surface.dispatchEvent(new MouseEvent('auxclick', {bubbles:true,cancelable:true,button:1}));
          const range = document.createRange();
          range.selectNodeContents(title);
          getSelection().removeAllRanges();
          getSelection().addRange(range);
          surface.dispatchEvent(new MouseEvent('click', {bubbles:true,cancelable:true}));
          getSelection().removeAllRanges();
          title.removeEventListener('click', capture);
          title.removeEventListener('auxclick', capture);
          const after = {row:row.getBoundingClientRect().left, title:title.getBoundingClientRect().left, rank:rank.getBoundingClientRect().left, background:getComputedStyle(row).backgroundColor};
          const marker = getComputedStyle(row, '::before');
          const original = row.querySelector('.u-bookmark-of'), field = original.parentElement;
          const linkRect = original.getBoundingClientRect(), fieldRect = field.getBoundingClientRect();
          const separator = getComputedStyle(field, '::before');
          const hit = document.elementFromPoint((fieldRect.left + linkRect.left) / 2, linkRect.top + linkRect.height / 2);
          return {forwarded, before, after, marker:{width:marker.width,opacity:marker.opacity,transition:marker.transitionDuration,display:marker.display}, separator:{outsideLink:!hit.closest('a'),balanced:separator.marginLeft===separator.marginRight,content:separator.content}, article:title.href, category:row.querySelector('.category a').href};
        "#, vec![]).await?;
        assert_eq!(
            row_actions["forwarded"],
            json!([
                {"type":"click","button":0,"ctrl":true,"shift":true},
                {"type":"click","button":1,"ctrl":false,"shift":false}
            ]),
            "background activation preserves link modifiers without navigating selected text"
        );
        assert_eq!(
            row_actions["before"], row_actions["after"],
            "selecting a row must keep its background and row, title, and rank positions unchanged"
        );
        assert_eq!(
            row_actions["marker"],
            json!({"width":"3px","opacity":"1","transition":"0s","display":if mobile {"none"} else {"block"}}),
            "the instant 3px selection marker is shown only on desktop"
        );
        assert_eq!(
            row_actions["separator"],
            json!({"outsideLink":!mobile,"balanced":true,"content":if mobile {"none"} else {"\"·\""}}),
            "desktop middots stay outside links; mobile uses spacing without wrapped leading dots"
        );
        client
            .find(Locator::Css(".row .category a"))
            .await?
            .click()
            .await?;
        wait_for(
            client,
            &format!("location.href === {}", row_actions["category"]),
        )
        .await?;
        client.goto(&fixture.base).await?;
        wait_for(
            client,
            "typeof window.swup?.navigate === 'function' && !!document.querySelector('.row .rank')",
        )
        .await?;
        client
            .find(Locator::Css(if mobile {
                ".row [data-row-open]"
            } else {
                ".row .rank"
            }))
            .await?
            .click()
            .await?;
        wait_for(
            client,
            &format!("location.href === {}", row_actions["article"]),
        )
        .await?;
    }
    client.goto(&fixture.base).await?;
    key(client, "g").await?;
    key(client, "l").await?;
    wait_for(client, "location.pathname === '/reader/browse/'").await?;
    client.goto(&fixture.base).await?;
    wait_for(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3 && document.documentElement.classList.contains('swup-enabled')",
    )
    .await?;
    client.execute("sessionStorage.setItem('aggr:last-seen-entry:' + encodeURIComponent('/reader/'), document.querySelectorAll('[data-row-open]')[1].href)", vec![]).await?;
    client.refresh().await?;
    wait_for(client, "!!document.querySelector('.row') && document.documentElement.classList.contains('swup-enabled')").await?;
    assert_eq!(
        client
            .execute(
                "return document.querySelectorAll('.row.is-selected').length",
                vec![]
            )
            .await?,
        1,
        "an ordinary reload selects the first visible row"
    );
    assert_eq!(
        client
            .execute(
                "return document.querySelectorAll('.new-marker').length",
                vec![]
            )
            .await?,
        0
    );
    let new_item_fade = client
        .execute_async(
            r#"
      const done = arguments[arguments.length - 1];
      const row = document.querySelector('.row');
      const cell = row.querySelector('.cell');
      const background = getComputedStyle(cell, '::before');
      const separator = getComputedStyle(row, '::after');
      const aligned = background.left === separator.left && background.right === separator.right;
      const color = () => getComputedStyle(cell, '::before').backgroundColor;
      const highlighted = color();
      const duration = background.transitionDuration;
      const badge = document.querySelector('link[rel~=icon]').href.startsWith('data:');
      requestAnimationFrame(() => {
        const initial = color();
        setTimeout(() => {
          const middle = color();
          setTimeout(() => {
            const final = color();
            done({highlighted, initial, middle, final, aligned, duration, badge});
          }, 3500);
        }, 2000);
      });
    "#,
            vec![],
        )
        .await?;
    assert_eq!(new_item_fade["duration"], "5s");
    assert_eq!(
        new_item_fade["badge"], false,
        "opening the page acknowledges the favicon badge"
    );
    assert_eq!(
        new_item_fade["aligned"], true,
        "the new-item highlight must share both horizontal edges with the separator"
    );
    assert_ne!(
        new_item_fade["initial"], new_item_fade["final"],
        "acknowledged entries must retain their highlight at the start of the fade"
    );
    assert_ne!(
        new_item_fade["middle"], new_item_fade["initial"],
        "the highlight should fade progressively"
    );
    assert_ne!(
        new_item_fade["middle"], new_item_fade["final"],
        "the highlight should remain partially visible midway through the fade"
    );
    key(client, "k").await?;
    let first = client
        .execute("return document.activeElement.href", vec![])
        .await?;
    assert_eq!(
        client
            .execute(
                "return getComputedStyle(document.activeElement).textDecorationLine",
                vec![]
            )
            .await?,
        "none",
        "the selected row marker is sufficient without underlining its title"
    );
    key(client, "j").await?;
    let second = client
        .execute("return document.activeElement.href", vec![])
        .await?;
    assert_ne!(first, second);
    key(client, "k").await?;
    assert_eq!(
        client
            .execute("return document.activeElement.href", vec![])
            .await?,
        first
    );
    client
        .execute("document.querySelector('.row .category a').focus()", vec![])
        .await?;
    key(client, "j").await?;
    assert_eq!(
        client
            .execute("return document.activeElement.href", vec![])
            .await?,
        second,
        "j must keep moving the feed cursor from a focused row metadata link"
    );
    key(client, "k").await?;
    assert_eq!(
        client
            .execute("return document.activeElement.href", vec![])
            .await?,
        first
    );
    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":390,"height":260,"deviceScaleFactor":1,"mobile":true}),
    )
    .await?;
    let scrolled = client
        .execute(
            r#"
      window.scrollTo(0, document.documentElement.scrollHeight);
      return {y:window.scrollY, max:document.documentElement.scrollHeight-document.documentElement.clientHeight};
    "#,
            vec![],
        )
        .await?;
    assert!(
        scrolled["y"].as_f64().unwrap_or_default() > 100.0,
        "fixture must exercise a genuinely scrolled feed: {scrolled}"
    );
    client
        .execute(
            r#"
      window.__aggrTransitionCalls = 0;
      if (document.startViewTransition) {
        const start = document.startViewTransition.bind(document);
        document.startViewTransition = function (update) {
          window.__aggrTransitionCalls++;
          return start(update);
        };
      }
    "#,
            vec![],
        )
        .await?;
    key(client, "o").await?;
    wait_for(client, "!!document.querySelector('.body pre')").await?;
    assert_eq!(
        client
            .execute("return window.__aggrTransitionCalls", vec![])
            .await?,
        0,
        "navigation should replace content without waiting for a page transition"
    );
    let scroll_samples = client
        .execute_async(
            r#"
      const done = arguments[arguments.length - 1];
      const samples = [];
      function sample() {
        samples.push(window.scrollY);
        if (samples.length === 10) done(samples);
        else requestAnimationFrame(sample);
      }
      requestAnimationFrame(sample);
    "#,
            vec![],
        )
        .await?;
    assert!(
        scroll_samples
            .as_array()
            .context("article scroll samples")?
            .iter()
            .all(|value| value.as_f64().unwrap_or_default().abs() <= 0.5),
        "article scroll position must stay at the top throughout the transition: {scroll_samples}"
    );
    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":1014,"height":700,"deviceScaleFactor":1,"mobile":false}),
    )
    .await?;
    client
        .execute_async(
            "const done=arguments[arguments.length-1];window.scrollTo(0,0);setTimeout(done,350);",
            vec![],
        )
        .await?;
    let pinned_header = client
        .execute_async(
            r#"
      const done = arguments[arguments.length - 1];
      window.scrollTo(0, 0);
      requestAnimationFrame(function () {
        const head = document.querySelector('.itemhead');
        const title = head.querySelector('h1');
        const main = document.querySelector('.main');
        const top = document.querySelector('.top');
        const headStyle = getComputedStyle(head);
        const titleStyle = getComputedStyle(title);
        const initial = {headTop:head.getBoundingClientRect().top,titleTop:title.getBoundingClientRect().top,titleLeft:title.getBoundingClientRect().left,
          mainTop:main.getBoundingClientRect().top,mainPadding:getComputedStyle(main).paddingTop,
          mainPaddingLeft:parseFloat(getComputedStyle(main).paddingLeft),
          headPaddingTop:parseFloat(headStyle.paddingTop),headPaddingBottom:parseFloat(headStyle.paddingBottom),
          titleMarginBottom:parseFloat(titleStyle.marginBottom),
          topHeight:top.getBoundingClientRect().height,topOffset:getComputedStyle(document.documentElement).getPropertyValue('--top-nav-offset')};
        window.scrollTo(0, 520);
        requestAnimationFrame(function () {
          requestAnimationFrame(function () {
            const stuck = {headTop:head.getBoundingClientRect().top,titleTop:title.getBoundingClientRect().top,titleLeft:title.getBoundingClientRect().left,scrollY:window.scrollY};
            window.scrollTo(0, 0);
            requestAnimationFrame(function () { done({initial,stuck}); });
          });
        });
      });
    "#,
            vec![],
        )
        .await?;
    assert!(
        pinned_header["stuck"]["scrollY"]
            .as_f64()
            .unwrap_or_default()
            > 100.0,
        "desktop fixture must scroll: {pinned_header}"
    );
    assert_eq!(
        pinned_header["initial"]["mainPaddingLeft"], 32.0,
        "desktop articles should keep a 32px reading gutter: {pinned_header}"
    );
    assert!(
        pinned_header["initial"]["headPaddingTop"]
            .as_f64()
            .unwrap_or_default()
            >= 32.0
            && pinned_header["initial"]["headPaddingBottom"]
                .as_f64()
                .unwrap_or_default()
                >= 14.0
            && pinned_header["initial"]["titleMarginBottom"]
                .as_f64()
                .unwrap_or_default()
                >= 12.0,
        "desktop article headers should have comfortable vertical spacing: {pinned_header}"
    );
    for axis in ["headTop", "titleLeft"] {
        let initial = pinned_header["initial"][axis]
            .as_f64()
            .context("initial pinned header coordinate")?;
        let stuck = pinned_header["stuck"][axis]
            .as_f64()
            .context("stuck pinned header coordinate")?;
        assert!(
            (initial - stuck).abs() <= 1.0,
            "the article header must not shift when it becomes sticky ({axis}): {pinned_header}"
        );
    }
    client
        .execute_async(
            "const done=arguments[arguments.length-1];window.scrollTo(0,0);setTimeout(done,350);",
            vec![],
        )
        .await?;
    let article_widths = client
        .execute(
            r#"
      return ['.itemhead', '.body', '.article-footer'].map(selector => {
        const rect = document.querySelector(selector).getBoundingClientRect();
        return {left:rect.left, width:rect.width};
      });
    "#,
            vec![],
        )
        .await?;
    assert_eq!(
        article_widths[0], article_widths[1],
        "header and prose must align"
    );
    assert_eq!(
        article_widths[1], article_widths[2],
        "both separators must span the same article width"
    );
    screenshot(client, "desktop-article-expanded").await?;
    client
        .execute_async(
            "const done=arguments[arguments.length-1];window.scrollTo(0,520);setTimeout(done,350);",
            vec![],
        )
        .await?;
    screenshot(client, "desktop-article-condensed").await?;
    client.execute("window.scrollTo(0,0)", vec![]).await?;
    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":390,"height":844,"deviceScaleFactor":1,"mobile":true}),
    )
    .await?;
    client
        .execute_async(
            "const done=arguments[arguments.length-1];window.scrollTo(0,0);setTimeout(done,350);",
            vec![],
        )
        .await?;
    let article = client.execute(r#"
      const body = document.querySelector('.body'), head = document.querySelector('.itemhead');
      const image = body.querySelector('img');
      const picture = image.closest('.article-picture');
      const loadedBox = picture.getBoundingClientRect();
      const unloaded = picture.cloneNode(true);
      unloaded.classList.remove('is-loaded');
      unloaded.removeAttribute('data-article-media-bound');
      unloaded.style.removeProperty('--image-preview');
      unloaded.querySelectorAll('source').forEach(source => source.removeAttribute('srcset'));
      unloaded.querySelector('img').removeAttribute('src');
      picture.after(unloaded);
      const unloadedBox = unloaded.getBoundingClientRect();
      unloaded.remove();
      const footer = document.querySelector('.article-footer').getBoundingClientRect();
      const more = document.querySelector('.article-more').getBoundingClientRect();
      const moreCards = Array.from(document.querySelectorAll('.article-more-link'));
      const moreBoxes = moreCards.map(card => card.getBoundingClientRect());
      return {code:body.querySelector('pre code').textContent, link:getComputedStyle(body.querySelector('p a')).display,
        headerMeta:head.querySelector('.meta').textContent.replace(/\s+/g, ' ').trim(),
        imageLoading:image.getAttribute('loading'), imageSource:image.getAttribute('src'), imageClass:image.className,
        imagePriority:image.getAttribute('fetchpriority'), imagePlaceholder:picture.style.getPropertyValue('--image-placeholder'),
        imagePreview:picture.style.getPropertyValue('--image-preview'), pictureLoaded:picture.classList.contains('is-loaded'),
        sourceWidth:picture.querySelector('source')?.getAttribute('width'), sourceHeight:picture.querySelector('source')?.getAttribute('height'),
        sourceSet:picture.querySelector('source')?.getAttribute('srcset'),
        loadedBox:{width:loadedBox.width,height:loadedBox.height}, unloadedBox:{width:unloadedBox.width,height:unloadedBox.height},
        imageWidth:image.naturalWidth, gap:body.getBoundingClientRect().top-head.getBoundingClientRect().bottom,
        mainPaddingLeft:parseFloat(getComputedStyle(document.querySelector('.main')).paddingLeft),
        readingStats:head.querySelector('.reading-stats')?.textContent.replace(/\s+/g, ' ').trim(),
        readingDuration:head.querySelector('.reading-stats time')?.getAttribute('datetime'),
        wordCount:head.querySelector('.reading-stats')?.title,
        publicationTitle:head.querySelector('.dt-published').closest('[data-date-tooltip]')?.title,
        timeTitle:head.querySelector('.dt-published').getAttribute('title'),
        linksTitled:Array.from(head.querySelectorAll('.u-bookmark-of, .discussion')).every(link => link.title === link.href),
        indent:parseFloat(getComputedStyle(body.querySelector('p')).textIndent),
        separateTags:!head.querySelector('.meta .item-tags'), metadataOrder:Array.from(head.querySelector('.meta').children).map(node => node.firstElementChild.className), categoryBelow:!!head.querySelector('.item-tags a[href*="categories/"]'), categoryText:head.querySelector('.meta .category')?.textContent, categoryUrl:head.querySelector('.meta .category a')?.pathname, categoryQuery:new URL(head.querySelector('.meta .category a').href).searchParams.get('q'), leadingRule:getComputedStyle(body.querySelector('hr:first-child')).display,
        outline:getComputedStyle(document.querySelector('main')).outlineStyle,
        highlighted:!!body.querySelector('pre span'), bodyLeft:body.getBoundingClientRect().left, bodyWidth:body.getBoundingClientRect().width,
        footerLeft:footer.left, footerWidth:footer.width, moreLeft:more.left, moreWidth:more.width,
        moreHeadings:[...document.querySelectorAll('.article-more-heading')].map(node => node.textContent),
        moreHeadingSize:parseFloat(getComputedStyle(document.querySelector('.article-more-heading')).fontSize),
        moreTitleSize:parseFloat(getComputedStyle(moreCards[0]).fontSize),
        moreMetadata:moreCards.every(link=>{const card=link.closest('.article-more-card');return ['.meta .domain','.meta .category a','.meta .published-date','.meta .reading-stats','.meta .u-bookmark-of'].every(selector=>card.querySelector(selector))}),
        moreCount:moreCards.length, moreClasses:Array.from(new Set(moreCards.map(card => card.className))).length,
        moreStacked:moreBoxes.length < 2 || moreBoxes[1].top > moreBoxes[0].bottom};
    "#, vec![]).await?;
    assert_eq!(article["code"], "$ z dotfiles\n$ pwd\n/private/dotfiles\n");
    assert_eq!(article["link"], "inline");
    assert!(
        !article["headerMeta"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase()
            .contains("published "),
        "article dates should use the same concise label as feed and search: {article}"
    );
    assert_eq!(
        article["metadataOrder"],
        json!([
            "domain",
            "category",
            "published-date",
            "reading-stats",
            "u-bookmark-of"
        ])
    );
    assert_eq!(article["categoryBelow"], false);
    assert_eq!(article["categoryText"], "/engineering");
    assert_eq!(article["categoryUrl"], "/reader/");
    assert_eq!(article["categoryQuery"], "category:\"engineering\"");
    assert_eq!(article["imageLoading"], "eager");
    assert_eq!(article["imagePriority"], "high");
    assert_eq!(article["imageClass"], "progressive-image");
    assert_eq!(article["imagePlaceholder"], "#315d76");
    assert!(
        article["imagePreview"]
            .as_str()
            .unwrap_or_default()
            .contains("data:image/png;base64,"),
        "{article}"
    );
    assert_eq!(article["pictureLoaded"], true);
    assert_eq!(article["sourceWidth"], Value::Null);
    assert_eq!(article["sourceHeight"], Value::Null);
    assert_eq!(
        article["sourceSet"],
        Value::Null,
        "a partial srcset must not make wide screens upscale a thumbnail: {article}"
    );
    assert_eq!(article["imageWidth"], 640);
    for axis in ["width", "height"] {
        let loaded = article["loadedBox"][axis].as_f64().unwrap_or_default();
        let unloaded = article["unloadedBox"][axis].as_f64().unwrap_or_default();
        assert!(
            loaded > 0.0 && (loaded - unloaded).abs() <= 0.5,
            "reserved image {axis} must match its loaded box: {article}"
        );
    }
    assert!(
        article["imageSource"]
            .as_str()
            .unwrap_or_default()
            .starts_with("assets/images/"),
        "{article}"
    );
    assert_eq!(article["highlighted"], true);
    assert_eq!(article["linksTitled"], true);
    assert_eq!(article["separateTags"], true);
    assert!(
        article["publicationTitle"]
            .as_str()
            .is_some_and(|value| value.contains("2026"))
    );
    assert_eq!(article["timeTitle"], Value::Null);
    assert!(
        article["publicationTitle"]
            .as_str()
            .is_some_and(|title| title.lines().count() == 2
                && title.contains("Published:")
                && title.contains("Updated:")),
        "both dates belong in the publication tooltip: {article}"
    );
    assert!(
        !article["headerMeta"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase()
            .contains("updated"),
        "updated timestamps must stay out of visible metadata: {article}"
    );
    assert_eq!(article["leadingRule"], "none");
    assert!(
        article["indent"].as_f64().unwrap_or_default() == 0.0,
        "paragraph indentation is off by default: {article}"
    );
    assert!(
        article["wordCount"]
            .as_str()
            .is_some_and(|text| text.ends_with(" words")),
        "word count should be available on hover: {article}"
    );
    assert_eq!(
        article["mainPaddingLeft"], 20.0,
        "mobile articles should keep a 20px reading gutter: {article}"
    );
    assert!(
        article["readingStats"]
            .as_str()
            .is_some_and(|text| !text.contains("words") && text.ends_with(" min read")),
        "reading time should appear without the word count: {article}"
    );
    assert!(
        article["readingDuration"]
            .as_str()
            .is_some_and(|duration| duration.starts_with("PT")
                && (duration.ends_with('M') || duration.ends_with('S'))),
        "reading time should carry a machine-readable duration: {article}"
    );
    assert!(
        article["gap"].as_f64().unwrap_or_default() >= 20.0,
        "{article}"
    );
    assert!(
        article["outline"] == "none" || article["outline"] == "hidden",
        "{article}"
    );
    assert!(
        (article["bodyLeft"].as_f64().unwrap_or_default()
            - article["footerLeft"].as_f64().unwrap_or_default())
        .abs()
            <= 1.0
            && (article["bodyWidth"].as_f64().unwrap_or_default()
                - article["footerWidth"].as_f64().unwrap_or_default())
            .abs()
                <= 1.0,
        "article content and continuation cards must share the same measure: {article}"
    );
    assert_eq!(article["moreCount"], 2);
    assert_eq!(
        article["moreHeadings"],
        json!(["Coming next", "Discover more"])
    );
    assert!(article["moreHeadingSize"].as_f64().unwrap_or_default() >= 16.0);
    assert!(article["moreTitleSize"].as_f64().unwrap_or_default() >= 16.0);
    assert_eq!(article["moreMetadata"], true);
    assert_eq!(article["moreClasses"], 1);
    assert_eq!(article["moreStacked"], true);
    client
        .execute_async(
            "const done=arguments[arguments.length-1];window.scrollTo(0,0);setTimeout(done,350);",
            vec![],
        )
        .await?;
    let sticky_head = client
        .execute_async(
            r#"
      const done = arguments[arguments.length - 1];
      window.scrollTo(0, 0);
      requestAnimationFrame(function () {
        const head = document.querySelector('.itemhead');
        const title = head.querySelector('h1');
        const initial = {top:head.getBoundingClientRect().top,titleTop:title.getBoundingClientRect().top,
          titleLeft:title.getBoundingClientRect().left,
          topBarBottom:document.querySelector('.top').getBoundingClientRect().bottom,
          paddingTop:parseFloat(getComputedStyle(head).paddingTop),
          position:getComputedStyle(head).position};
        window.scrollTo(0, 520);
        requestAnimationFrame(function () {
          requestAnimationFrame(function () {
            const stuck = {top:head.getBoundingClientRect().top,titleTop:title.getBoundingClientRect().top,
              titleLeft:title.getBoundingClientRect().left,scrollY:window.scrollY};
            window.scrollTo(0, 0);
            requestAnimationFrame(function () { done({initial,stuck}); });
          });
        });
      });
    "#,
            vec![],
        )
        .await?;
    assert_eq!(sticky_head["initial"]["position"], "sticky");
    assert_eq!(
        sticky_head["initial"]["paddingTop"], 24.0,
        "mobile article headers should retain a 24px top gutter: {sticky_head}"
    );
    assert!(
        (sticky_head["initial"]["top"].as_f64().unwrap_or(f64::MAX)
            - sticky_head["initial"]["topBarBottom"]
                .as_f64()
                .unwrap_or_default())
        .abs()
            <= 1.0,
        "the article title and metadata must remain fixed immediately below the mobile top bar: {sticky_head}"
    );
    for axis in ["top", "titleLeft"] {
        let initial = sticky_head["initial"][axis]
            .as_f64()
            .context("initial mobile sticky-header coordinate")?;
        let stuck = sticky_head["stuck"][axis]
            .as_f64()
            .context("scrolled mobile sticky-header coordinate")?;
        assert!(
            (initial - stuck).abs() <= 1.0,
            "the mobile article header must not jump while becoming sticky: {sticky_head}"
        );
    }
    let reading_header = client.execute_async(r#"
      const done = arguments[arguments.length - 1], head = document.querySelector('.itemhead');
      const tags = head.querySelector('.item-tags');
      async function sample(y) {
        window.scrollTo(0,y);
        await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        const title=head.querySelector('.itemhead-title');
        return {size:parseFloat(getComputedStyle(title).fontSize) * new DOMMatrixReadOnly(getComputedStyle(title).transform).a,
          opacity:parseFloat(getComputedStyle(tags).opacity), height:tags.getBoundingClientRect().height,
          metadata:getComputedStyle(head.querySelector('.meta')).display,
          separate:tags.getBoundingClientRect().top >= head.querySelector('.meta').getBoundingClientRect().bottom,
          offset:parseFloat(getComputedStyle(document.documentElement).scrollPaddingTop), bottom:head.getBoundingClientRect().bottom};
      }
      (async () => done({expanded:await sample(0),quarter:await sample(40),half:await sample(80),
        condensed:await sample(520),reverseHalf:await sample(80),restored:await sample(0)}))();
    "#, vec![]).await?;
    assert_eq!(reading_header["expanded"]["separate"], true);
    for (before, after) in [
        ("expanded", "quarter"),
        ("quarter", "half"),
        ("half", "condensed"),
    ] {
        for field in ["size", "opacity", "height"] {
            assert!(
                reading_header[before][field].as_f64() > reading_header[after][field].as_f64(),
                "header must fold continuously ({field}): {reading_header}"
            );
        }
    }
    assert_eq!(
        reading_header["half"]["size"],
        reading_header["reverseHalf"]["size"]
    );
    assert_eq!(
        reading_header["expanded"]["size"],
        reading_header["restored"]["size"]
    );
    assert_eq!(reading_header["condensed"]["height"], 0.0);
    assert_ne!(reading_header["condensed"]["metadata"], "none");
    assert!(
        reading_header["condensed"]["offset"].as_f64()
            > reading_header["condensed"]["bottom"].as_f64()
    );
    let title_fold = client.execute_async(r#"
      const done=arguments[arguments.length-1], head=document.querySelector('.itemhead');
      const title=head.querySelector('.itemhead-title') || head.querySelector('h1');
      const original=title.textContent;
      head.style.width='550px';
      title.textContent='Proactive cyber defense for governments and enterprises';
      const frame=()=>new Promise(resolve=>requestAnimationFrame(resolve));
      (async()=>{
        window.scrollTo(0,0); window.dispatchEvent(new Event('resize'));
        await frame(); await frame(); await frame();
        const samples=[];
        for(let y=0;y<=160;y+=4){
          window.scrollTo(0,y); await frame(); await frame();
          samples.push(head.getBoundingClientRect().height);
        }
        const largestStep=Math.max(...samples.slice(1).map((height,i)=>Math.abs(height-samples[i])));
        title.textContent=original; head.style.removeProperty('width'); window.scrollTo(0,0); window.dispatchEvent(new Event('resize'));
        await frame(); await frame(); await frame();
        done({largestStep,expanded:samples[0],folded:samples.at(-1)});
      })();
    "#,vec![]).await?;
    assert!(
        title_fold["largestStep"].as_f64().unwrap_or(f64::MAX) < 4.0,
        "a wrapping title must not make the folding header jump: {title_fold}"
    );
    assert!(title_fold["expanded"].as_f64() > title_fold["folded"].as_f64());
    key(client, "G").await?;
    wait_for(client,"scrollY > 100 && Math.abs(scrollY + innerHeight - document.documentElement.scrollHeight) < 2").await?;
    key(client, "g").await?;
    key(client, "g").await?;
    wait_for(client, "scrollY === 0").await?;
    let reading_progress = client
        .execute_async(
            r#"
      const done = arguments[arguments.length - 1], head = document.querySelector('.itemhead');
      const frame = () => new Promise(resolve => requestAnimationFrame(resolve));
      async function sample(fraction) {
        window.scrollTo(0, 200);
        await frame(); await frame(); await frame();
        window.scrollTo(0, fraction * (document.documentElement.scrollHeight - innerHeight));
        await frame(); await frame(); await frame();
        const indicator = head.querySelector('.itemhead-progress'), line = getComputedStyle(indicator);
        return {progress:new DOMMatrixReadOnly(line.transform).a,
          fill:indicator.getBoundingClientRect().width / head.getBoundingClientRect().width, transition:line.transitionDuration,
          expected:scrollY / (document.documentElement.scrollHeight - innerHeight)};
      }
      (async () => done({top:await sample(0),quarter:await sample(.25),half:await sample(.5),
        bottom:await sample(1),reverse:await sample(.5),restored:await sample(0)}))();
    "#,
            vec![],
        )
        .await?;
    for (sample, expected) in [
        ("top", 0.0),
        ("quarter", 0.25),
        ("half", 0.5),
        ("bottom", 1.0),
        ("reverse", 0.5),
        ("restored", 0.0),
    ] {
        let progress = reading_progress[sample]["progress"]
            .as_f64()
            .context("reading progress")?;
        let fill = reading_progress[sample]["fill"]
            .as_f64()
            .context("progress line scale")?;
        assert!(
            (progress - expected).abs() < 0.002 && (fill - progress).abs() < 0.00001,
            "the header separator must fill linearly in both directions: {reading_progress}"
        );
        assert_eq!(reading_progress[sample]["transition"], "0s");
    }
    client.execute("window.scrollTo(0,520)", vec![]).await?;
    let header_reads = client
        .execute_async(
            r#"
      const done = arguments[arguments.length - 1], head = document.querySelector('.itemhead');
      const frame = () => new Promise(resolve => requestAnimationFrame(resolve));
      (async () => {
        await frame(); await frame(); await frame();
        const original = head.getBoundingClientRect;
        let reads = 0;
        head.getBoundingClientRect = function () { reads++; return original.call(this); };
        for (const y of [600, 700, 800, 900]) { scrollTo(0,y); await frame(); await frame(); }
        head.getBoundingClientRect = original;
        scrollTo(0,520);
        done(reads);
      })();
    "#,
            vec![],
        )
        .await?;
    assert_eq!(
        header_reads, 0,
        "folded scrolling should reuse measured header geometry"
    );
    screenshot(client, "mobile-article-folded").await?;
    client.execute("window.scrollTo(0,0)", vec![]).await?;
    wait_for(client, "scrollY === 0").await?;
    screenshot(client, "mobile-article").await?;
    client.back().await?;
    wait_for(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3",
    )
    .await?;
    wait_for(
        client,
        "!!document.activeElement.matches('[data-row-open]')",
    )
    .await?;
    assert_eq!(
        client
            .execute("return document.activeElement.href", vec![])
            .await?,
        first
    );

    client
        .goto(first.as_str().context("selected article URL")?)
        .await?;
    wait_for(client, "!!document.querySelector('article.item')").await?;
    client
        .execute(
            "window.__aggrOpened=[];window.open=function(url){window.__aggrOpened.push(url);return {};};",
            vec![],
        )
        .await?;
    for key_name in ["O", "H", "R", "X"] {
        key(client, key_name).await?;
    }
    let external_shortcuts = client
        .execute(
            r#"
      return {enabled:window.AGGR.discussions.map(network => network.shortcut),
        opened:window.__aggrOpened.map(value => { const url=new URL(value); return {host:url.host,query:url.search}; })};
    "#,
            vec![],
        )
        .await?;
    assert_eq!(external_shortcuts["enabled"], json!(["H", "R"]));
    assert_eq!(
        external_shortcuts["opened"].as_array().map(Vec::len),
        Some(3)
    );
    assert_eq!(external_shortcuts["opened"][0]["host"], "publisher.invalid");
    assert_eq!(external_shortcuts["opened"][1]["host"], "hn.algolia.com");
    assert!(
        external_shortcuts["opened"][1]["query"]
            .as_str()
            .unwrap_or_default()
            .contains("publisher.invalid%2Fstory-45"),
        "{external_shortcuts}"
    );
    assert_eq!(external_shortcuts["opened"][2]["host"], "www.reddit.com");
    let scroll_shortcuts = client
        .execute(
            r#"
      const movements = [], prevented = {};
      const scrollBy = window.scrollBy;
      window.scrollBy = (options) => movements.push(options.top);
      function press(label, key, options = {}, target = document.body) {
        const event = new KeyboardEvent('keydown', {key, bubbles:true, cancelable:true, ...options});
        target.dispatchEvent(event);
        prevented[label] = event.defaultPrevented;
      }
      press('d', 'd'); press('ctrlD', 'd', {ctrlKey:true});
      press('u', 'u'); press('ctrlU', 'u', {ctrlKey:true});
      press('ctrlE', 'e', {ctrlKey:true}); press('ctrlY', 'y', {ctrlKey:true});
      press('metaD', 'd', {metaKey:true}); press('ctrlShiftD', 'D', {ctrlKey:true, shiftKey:true});
      const input = document.createElement('textarea'); document.body.append(input);
      press('editing', 'u', {ctrlKey:true}, input); input.remove();
      const dialog = document.querySelector('#shortcut-help'); dialog.showModal();
      press('dialog', 'd', {ctrlKey:true}); dialog.close(); document.querySelector('#swup').focus({preventScroll:true});
      const preference = window.AGGRPreferences.values['single-key-shortcuts'];
      window.AGGRPreferences.values['single-key-shortcuts'] = false;
      press('disabledD', 'd'); press('enabledCtrlD', 'd', {ctrlKey:true});
      window.AGGRPreferences.values['single-key-shortcuts'] = preference;
      window.scrollBy = scrollBy;
      const line = parseFloat(getComputedStyle(document.querySelector('.body')).lineHeight);
      const head = document.querySelector('.itemhead').getBoundingClientRect();
      const bottom = 0;
      return {movements, prevented, line, halfPage:Math.min(line * 10, Math.max(line, innerHeight - head.bottom - bottom) * 0.5)};
    "#,
            vec![],
        )
        .await?;
    let half_page = scroll_shortcuts["halfPage"]
        .as_f64()
        .context("reading scroll distance")?;
    let line = scroll_shortcuts["line"]
        .as_f64()
        .context("reading line height")?;
    assert_eq!(
        scroll_shortcuts["movements"],
        json!([
            half_page, half_page, -half_page, -half_page, line, -line, half_page
        ])
    );
    for name in ["d", "ctrlD", "u", "ctrlU", "ctrlE", "ctrlY", "enabledCtrlD"] {
        assert_eq!(
            scroll_shortcuts["prevented"][name], true,
            "{name}: {scroll_shortcuts}"
        );
    }
    for name in ["metaD", "ctrlShiftD", "editing", "dialog", "disabledD"] {
        assert_eq!(
            scroll_shortcuts["prevented"][name], false,
            "{name}: {scroll_shortcuts}"
        );
    }
    let article_paths = client
        .execute(
            r#"
      const article = document.querySelector('article.item');
      return {current:location.pathname,older:new URL(article.dataset.nextUrl,document.baseURI).pathname};
    "#,
            vec![],
        )
        .await?;
    let current_path = article_paths["current"]
        .as_str()
        .context("current article path")?
        .to_string();
    let older_path = article_paths["older"]
        .as_str()
        .context("older article path")?
        .to_string();
    for key_name in ["h", "l", "ArrowLeft", "ArrowRight"] {
        key(client, key_name).await?;
        assert_eq!(
            client.current_url().await?.path(),
            current_path,
            "{key_name} must not navigate between articles"
        );
    }
    key(client, "k").await?;
    assert_eq!(
        client.current_url().await?.path(),
        current_path,
        "k at the newest-article boundary must be a no-op"
    );
    client
        .execute("document.querySelector('.body a').focus()", vec![])
        .await?;
    assert_eq!(
        client
            .execute(
                "return document.activeElement.closest('.body') !== null",
                vec![]
            )
            .await?,
        true,
        "the keyboard contract must be exercised from a focused article link"
    );
    key(client, "j").await?;
    wait_for(
        client,
        &format!(
            "location.pathname === {} && !!document.querySelector('article.item')?.dataset.previousUrl && !document.documentElement.classList.contains('is-changing')",
            serde_json::to_string(&older_path)?
        ),
    )
    .await?;
    assert_eq!(
        client
            .execute(
                "return new URL(document.querySelector('article.item').dataset.previousUrl,document.baseURI).pathname",
                vec![]
            )
            .await?,
        current_path,
        "the older article must link back to the article opened from the feed"
    );
    client
        .execute("document.querySelector('.itemhead a').focus()", vec![])
        .await?;
    key(client, "k").await?;
    wait_for(
        client,
        &format!(
            "location.pathname === {} && !!document.querySelector('article.item') && !document.documentElement.classList.contains('is-changing')",
            serde_json::to_string(&current_path)?
        ),
    )
    .await?;
    client.goto(&fixture.base).await?;
    wait_for(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3",
    )
    .await?;

    for key_name in ["h", "l", "ArrowLeft", "ArrowRight"] {
        key(client, key_name).await?;
        assert_eq!(
            client.current_url().await?.path(),
            "/reader/",
            "{key_name} must not turn list pages"
        );
    }
    client
        .find(Locator::Css("[data-page-next]"))
        .await?
        .click()
        .await?;
    wait_for(client, "location.pathname.includes('/page/2/')").await?;
    wait_for(client, "document.querySelector('[data-row-open]').href.includes('story-42/') && !document.documentElement.classList.contains('is-changing')").await?;
    key(client, "j").await?;
    client.back().await?;
    wait_for(
        client,
        "location.pathname === '/reader/' && document.querySelector('[data-row-open]')?.href.includes('story-45/') && !!document.activeElement.matches('[data-row-open]') && !document.documentElement.classList.contains('is-changing')",
    )
    .await?;
    assert_eq!(
        client
            .execute("return document.activeElement.href", vec![])
            .await?,
        first
    );

    client
        .goto(&format!("{}preferences/", fixture.base))
        .await?;
    wait_for(
        client,
        "document.querySelectorAll('[data-preference]').length === 18",
    )
    .await?;
    let preference_controls = client
        .execute(
            r#"
      return {
        ids:Array.from(document.querySelectorAll('[data-preference]')).map(control => control.id),
        groups:Array.from(document.querySelectorAll('.preferences-group:not(.preferences-import) > h2')).map(heading => heading.textContent),
        install:!!document.querySelector('#install-app'),
        config:!!document.querySelector('.preferences-config'),
        shortcutLink:document.querySelector('#show-shortcuts')?.tagName,
        defaults:Object.fromEntries(Array.from(document.querySelectorAll('[data-preference]')).map(control => [control.dataset.preference, control.type === 'checkbox' ? control.checked : control.value]))
      };
    "#,
            vec![],
        )
        .await?;
    assert_eq!(
        preference_controls["ids"],
        json!([
            "theme-mode",
            "motion",
            "font-family",
            "text-size",
            "reading-width",
            "line-spacing",
            "paragraph-spacing",
            "paragraph-indent",
            "text-align",
            "letter-spacing",
            "word-spacing",
            "density",
            "thumbnails",
            "feed-page-size",
            "date-format",
            "scroll-amount",
            "single-key-shortcuts",
            "offline-items"
        ])
    );
    assert_eq!(
        preference_controls["groups"],
        json!([
            "Appearance",
            "Reading",
            "Feed",
            "Keyboard",
            "Offline reading"
        ])
    );
    assert_eq!(
        preference_controls["defaults"]["single-key-shortcuts"],
        true
    );
    assert_eq!(preference_controls["defaults"]["feed-page-size"], "50");
    assert_eq!(preference_controls["install"], false);
    assert_eq!(preference_controls["config"], false);
    assert_eq!(preference_controls["shortcutLink"], "A");
    client
        .execute(
            "document.querySelector('#show-shortcuts').scrollIntoView({block:'center'})",
            vec![],
        )
        .await?;
    client
        .find(Locator::Css("#show-shortcuts"))
        .await?
        .click()
        .await?;
    wait_for(client, "document.querySelector('#shortcut-help').open").await?;
    assert_eq!(
        client.current_url().await?.path(),
        "/reader/preferences/",
        "the shortcut helper link must stay on Preferences"
    );
    assert_eq!(
        client
            .execute(
                "return {checked:document.querySelector('#single-key-shortcuts').checked, decoration:getComputedStyle(document.querySelector('#show-shortcuts')).textDecorationLine}",
                vec![]
            )
            .await?,
        json!({"checked":true,"decoration":"underline"})
    );
    key(client, "Escape").await?;
    let applied_preferences = client
        .execute(
            r#"
      const values = {
        'theme-mode':'dark', motion:'off', 'text-size':'large', 'reading-width':'wide',
        density:'comfortable', thumbnails:'hide', 'date-format':'iso'
      };
      Object.keys(values).forEach(function (id) {
        const control = document.getElementById(id);
        control.value = values[id];
        control.dispatchEvent(new Event('change', {bubbles:true}));
      });
      return {
        theme:document.documentElement.dataset.theme,
        motion:document.documentElement.dataset.motion,
        textSize:document.documentElement.dataset.textSize,
        readingWidth:document.documentElement.dataset.readingWidth,
        density:document.documentElement.dataset.density,
        thumbnails:document.documentElement.dataset.thumbnails,
        date:localStorage.getItem('aggr:date-format')
      };
    "#,
            vec![],
        )
        .await?;
    assert_eq!(
        applied_preferences,
        json!({"theme":"dark","motion":"off","textSize":"large","readingWidth":"wide",
          "density":"comfortable","thumbnails":"hide","date":"iso"})
    );
    screenshot(client, "mobile-preferences").await?;

    client.goto(&fixture.base).await?;
    wait_for(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3 && document.documentElement.dataset.density === 'comfortable'",
    )
    .await?;
    let comfortable_row = client
        .execute(
            r#"
      const row = document.querySelectorAll('.row')[1];
      const title = row.querySelector('.title');
      row.classList.remove('is-selected');
      const plain = getComputedStyle(row).backgroundColor;
      const plainInset = title.getBoundingClientRect().left-row.getBoundingClientRect().left;
      row.classList.add('is-selected');
      return {
        height:row.getBoundingClientRect().height,
        inset:title.getBoundingClientRect().left-row.getBoundingClientRect().left,
        plainInset,
        contentInset:parseFloat(getComputedStyle(row).paddingLeft),
        padding:parseFloat(getComputedStyle(row).paddingTop),
        plain:plain,
        selected:getComputedStyle(row).backgroundColor
      };
    "#,
            vec![],
        )
        .await?;
    assert!(
        comfortable_row["height"].as_f64().unwrap_or_default() >= compact_row_height + 15.0,
        "comfortable density should add deliberate space: compact={compact_row_height}, comfortable={comfortable_row}"
    );
    assert!(
        comfortable_row["padding"].as_f64().unwrap_or_default() >= 9.5
            && (comfortable_row["inset"].as_f64().unwrap_or_default()
                - comfortable_row["contentInset"].as_f64().unwrap_or_default())
            .abs()
                <= 1.0
            && comfortable_row["inset"] == comfortable_row["plainInset"],
        "comfortable selected rows need padding without losing alignment: {comfortable_row}"
    );
    assert_eq!(
        comfortable_row["plain"], comfortable_row["selected"],
        "selection must leave the row background unchanged at every density"
    );

    client
        .goto(&format!("{}preferences/", fixture.base))
        .await?;
    wait_for(
        client,
        "document.querySelectorAll('[data-preference]').length === 18",
    )
    .await?;
    client
        .execute(
            "const control=document.querySelector('#single-key-shortcuts');control.checked=false;control.dispatchEvent(new Event('change',{bubbles:true}));",
            vec![],
        )
        .await?;

    client
        .execute(
            "Object.defineProperty(Navigator.prototype, 'clipboard', {configurable:true, get:function () { return undefined; }});",
            vec![],
        )
        .await?;
    client
        .execute("document.querySelector('#copy-state').click();", vec![])
        .await?;
    wait_for(
        client,
        "!document.querySelector('#preferences-link').hidden && document.querySelector('#preferences-link').value.includes('#aggr-state=')",
    )
    .await?;
    let copied_link = client
        .find(Locator::Css("#preferences-link"))
        .await?
        .prop("value")
        .await?
        .context("copied preferences link")?;
    let copied_link_contract = client
        .execute(
            r#"
      const url = new URL(document.querySelector('#preferences-link').value);
      return {path:url.pathname, fragment:url.hash.startsWith('#aggr-state='), query:url.searchParams.has('aggr-state')};
    "#,
            vec![],
        )
        .await?;
    assert_eq!(
        copied_link_contract,
        json!({"path":"/reader/preferences/","fragment":true,"query":false})
    );

    let ignored_transfer = client.execute(r#"
      const url=new URL('preferences/',document.baseURI);
      url.searchParams.set('aggr-state',btoa(JSON.stringify({version:1,preferences:{theme:'light'}})));
      return url.href;
    "#,vec![]).await?.as_str().context("query-string transfer URL")?.to_owned();
    client.goto(&ignored_transfer).await?;
    wait_for(
        client,
        "document.querySelector('#theme-mode') && document.querySelector('#preferences-import')",
    )
    .await?;
    anyhow::ensure!(client.execute("return document.documentElement.dataset.theme==='dark' && document.querySelector('#theme-mode').value==='dark' && document.querySelector('#preferences-import').hidden",vec![]).await?==true,"query-string preference transfers are ignored without opening a review or changing settings");

    let valid_preferences = fixture._directory.path().join("valid-preferences.json");
    let invalid_preferences = fixture._directory.path().join("invalid-preferences.json");
    std::fs::write(
        &valid_preferences,
        r#"{"version":1,"preferences":{"theme":"light","density":"compact"}}"#,
    )?;
    std::fs::write(
        &invalid_preferences,
        r#"{"version":1,"preferences":{"theme":"ultraviolet"}}"#,
    )?;
    client
        .find(Locator::Css("#preferences-file"))
        .await?
        .send_keys(
            valid_preferences
                .to_str()
                .context("preferences file path")?,
        )
        .await?;
    wait_for(
        client,
        "!document.querySelector('#preferences-import').hidden && document.querySelectorAll('#preferences-import-summary li').length === 2",
    )
    .await?;
    let before_apply = client
        .execute(
            "return {theme:document.documentElement.dataset.theme,density:document.documentElement.dataset.density,storedTheme:localStorage.getItem('aggr:theme'),storedDensity:localStorage.getItem('aggr:density')};",
            vec![],
        )
        .await?;
    assert_eq!(
        before_apply,
        json!({"theme":"dark","density":"comfortable","storedTheme":"dark","storedDensity":"comfortable"}),
        "import review must not mutate preferences"
    );
    client
        .execute(
            "document.querySelector(\"[data-preferences-action='apply']\").click();",
            vec![],
        )
        .await?;
    wait_for(
        client,
        "document.documentElement.dataset.theme === 'light' && document.documentElement.dataset.density === 'compact' && document.querySelector('#preferences-import').hidden",
    )
    .await?;

    client.goto(&fixture.base).await?;
    client.goto(&copied_link).await?;
    wait_for(
        client,
        "!document.querySelector('#preferences-import').hidden && document.querySelectorAll('#preferences-import-summary li').length === 18",
    )
    .await?;
    assert_eq!(
        client
            .execute(
                "return {theme:document.documentElement.dataset.theme,density:document.documentElement.dataset.density,fragment:location.hash};",
                vec![]
            )
            .await?,
        json!({"theme":"light","density":"compact","fragment":""}),
        "fragment transfer must be reviewed and removed from browser history before applying"
    );
    client
        .execute(
            "document.querySelector(\"[data-preferences-action='apply']\").click();",
            vec![],
        )
        .await?;
    wait_for(
        client,
        "document.documentElement.dataset.theme === 'dark' && document.documentElement.dataset.density === 'comfortable'",
    )
    .await?;

    client.goto(&fixture.base).await?;
    client
        .goto(&format!(
            "{}preferences/#aggr-state=not_valid!",
            fixture.base
        ))
        .await?;
    wait_for(
        client,
        "document.querySelector('#preferences-status').textContent.includes('invalid or unsupported')",
    )
    .await?;
    assert_eq!(
        client
            .execute(
                "return {theme:document.documentElement.dataset.theme,density:document.documentElement.dataset.density,panel:document.querySelector('#preferences-import').hidden};",
                vec![]
            )
            .await?,
        json!({"theme":"dark","density":"comfortable","panel":true}),
        "invalid fragments must leave the stored state untouched"
    );
    client
        .find(Locator::Css("#preferences-file"))
        .await?
        .send_keys(
            invalid_preferences
                .to_str()
                .context("invalid preferences file path")?,
        )
        .await?;
    wait_for(
        client,
        "document.querySelector('#preferences-status').textContent.includes('file is invalid or unsupported')",
    )
    .await?;
    assert_eq!(
        client
            .execute(
                "return {theme:document.documentElement.dataset.theme,density:document.documentElement.dataset.density,panel:document.querySelector('#preferences-import').hidden};",
                vec![]
            )
            .await?,
        json!({"theme":"dark","density":"comfortable","panel":true}),
        "invalid files must leave the stored state untouched"
    );

    client
        .find(Locator::Css("#preferences-file"))
        .await?
        .send_keys(
            valid_preferences
                .to_str()
                .context("preferences file path")?,
        )
        .await?;
    wait_for(
        client,
        "!document.querySelector('#preferences-import').hidden",
    )
    .await?;
    client.execute("document.querySelector('[data-preferences-action=cancel]').scrollIntoView({block:'nearest',behavior:'instant'})", vec![]).await?;
    assert_eq!(client.execute("return document.querySelector('[data-preferences-action=cancel]').getBoundingClientRect().bottom < document.querySelector('.mobile-tabs').getBoundingClientRect().top", vec![]).await?, true, "native scrolling must keep preference controls above the fixed tab bar");
    client
        .find(Locator::Css("[data-preferences-action='cancel']"))
        .await?
        .click()
        .await?;
    wait_for(
        client,
        "document.querySelector('#preferences-status').textContent.includes('Import cancelled')",
    )
    .await?;
    std::fs::write(
        &invalid_preferences,
        r#"{"aggr:theme":"light","aggr:density":"compact"}"#,
    )?;
    client
        .find(Locator::Css("#preferences-file"))
        .await?
        .send_keys(
            invalid_preferences
                .to_str()
                .context("obsolete preference file path")?,
        )
        .await?;
    wait_for(client,"document.querySelector('#preferences-status').textContent.includes('file is invalid or unsupported')").await?;
    anyhow::ensure!(client.execute("return document.documentElement.dataset.theme==='dark' && document.documentElement.dataset.density==='comfortable' && document.querySelector('#preferences-import').hidden",vec![]).await?==true,"unversioned storage-key exports are rejected without changing preferences");

    client.goto(&fixture.base).await?;
    wait_for(client, "!!document.querySelector('[data-row-open]')").await?;
    client
        .execute(
            "const row=document.querySelector('.row');row.classList.add('is-selected');row.querySelector('[data-row-open]').focus({preventScroll:true});",
            vec![],
        )
        .await?;
    let cursor = client
        .execute(
            "return document.querySelector('.row.is-selected [data-row-open]')?.href || null",
            vec![],
        )
        .await?;
    key(client, "j").await?;
    assert_eq!(
        client
            .execute(
                "return document.querySelector('.row.is-selected [data-row-open]')?.href || null",
                vec![]
            )
            .await?,
        cursor
    );

    wait_for(client, "!!navigator.serviceWorker.controller").await?;
    let checks = client
        .execute_async(
            include_str!("fixtures/service_worker_checks.js"),
            vec![json!(std::fs::read_to_string(fixture.out.join("sw.js"))?)],
        )
        .await?;
    assert!(checks.get("error").is_none(), "{checks}");
    assert_eq!(checks["checks"], 55);
    let identities = client
        .execute_async(
            r#"
      const done = arguments[arguments.length-1];
      fetch('manifest.webmanifest').then(r=>r.json()).then(manifest=>done({id:manifest.id || null,
        one:new URL(manifest.start_url, location.origin+'/reader/manifest.webmanifest').href,
        two:new URL(manifest.start_url, location.origin+'/other/manifest.webmanifest').href}));
    "#,
            vec![],
        )
        .await?;
    assert_eq!(identities["id"], Value::Null);
    assert_ne!(identities["one"], identities["two"]);

    client
        .execute(
            "localStorage.setItem('aggr:single-key-shortcuts','true');localStorage.setItem('aggr:feed-page-size','25')",
            vec![],
        )
        .await?;
    client.goto(&format!("{}?q=article", fixture.base)).await?;
    wait_for(
        client,
        "document.querySelectorAll('#list .row').length === 25",
    )
    .await?;
    assert_eq!(
        client
            .execute(
                "return new URL(history.state.url,location.href).href===location.href",
                vec![]
            )
            .await?,
        true,
        "canonical search must keep Swup's history URL aligned with the address bar"
    );
    let input = client.find(Locator::Css("#q")).await?;
    input.clear().await?;
    input.send_keys("j").await?;
    assert_eq!(input.prop("value").await?, Some("j".into()));
    input.clear().await?;
    input.send_keys("article").await?;
    wait_for(
        client,
        "document.querySelector('#q').value === 'article' && document.querySelector('#list')?.getAttribute('aria-busy') === 'false' && document.querySelectorAll('#list .row').length === 25",
    )
    .await?;
    assert_eq!(client.execute("return document.querySelector('#list .row.is-selected') === document.querySelector('#list .row') && document.activeElement.id === 'q'", vec![]).await?, true,
        "search results select the first item while leaving typing focus in search");
    assert_eq!(client.execute(r#"
      const rows = [...document.querySelectorAll('#list .row')];
      const edge = rows[0].getBoundingClientRect().left + parseFloat(getComputedStyle(rows[0], '::after').left);
      return rows.every(row => {
        const separator = row.getBoundingClientRect().left + parseFloat(getComputedStyle(row, '::after').left);
        const marker = getComputedStyle(row, '::before');
        const markerRight = row.getBoundingClientRect().left + parseFloat(marker.left) + parseFloat(marker.width);
        return Math.abs(edge - separator) < 1 && Math.abs(markerRight - separator) < 1;
      });
    "#, vec![]).await?, true, "all separators must share one left edge and meet the selection bar");
    client
        .execute("document.querySelector('[data-row-open]').focus()", vec![])
        .await?;
    key(client, "j").await?;
    let selected_search = client
        .execute("return document.activeElement.href", vec![])
        .await?;
    key(client, "Enter").await?;
    wait_for(client, "!!document.querySelector('.body pre')").await?;
    assert_eq!(
        client.current_url().await?.as_str(),
        selected_search.as_str().unwrap()
    );
    client.back().await?;
    wait_for(client, "document.querySelectorAll('#list .row').length === 25 && !!document.activeElement.matches('[data-row-open]')").await?;
    assert_eq!(
        client
            .execute("return document.activeElement.href", vec![])
            .await?,
        selected_search
    );

    client
        .execute(
            "localStorage.setItem('aggr:theme','light');localStorage.setItem('aggr:density','compact');localStorage.setItem('aggr:text-size','default')",
            vec![],
        )
        .await?;
    for width in [320, 375, 430, 768, 1280] {
        emulate(
            client,
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":width<640}),
        )
        .await?;
        client.goto(&fixture.base).await?;
        let geometry = client
            .execute(
                r#"
          return {overflow:document.documentElement.scrollWidth > document.documentElement.clientWidth+1,
            searches:document.querySelectorAll('#q').length};
        "#,
                vec![],
            )
            .await?;
        assert_eq!(
            geometry,
            json!({"overflow":false,"searches":1}),
            "width={width}"
        );
    }
    emulate(
        client,
        "Emulation.setTouchEmulationEnabled",
        json!({"enabled":false}),
    )
    .await?;
    client.goto(&fixture.base).await?;
    assert_eq!(
        client
            .execute("return window.swup.options.native", vec![])
            .await?,
        false,
        "desktop navigation must also skip full-page transition snapshots"
    );
    let desktop_density = client
        .execute(
            r#"
      const rows = Array.from(document.querySelectorAll('.row')).slice(1);
      const row = rows[0];
      const title = row.querySelector('.title').getBoundingClientRect();
      const metadata = row.querySelector('.meta').getBoundingClientRect();
      const meta = row.querySelector('.meta > *').getBoundingClientRect();
      const rank = row.querySelector('.rank');
      const rankText = document.createRange();
      rankText.selectNodeContents(rank);
      const top = document.querySelector('.top');
      const topStyle = getComputedStyle(top);
      const topNavStyle = getComputedStyle(document.querySelector('.nav'));
      const brandLinkStyle = getComputedStyle(document.querySelector('.brand'));
      const configStyle = getComputedStyle(document.querySelector('.config-link'));
      const separatorLeft = row.getBoundingClientRect().left + parseFloat(getComputedStyle(row, '::after').left);
      return {height:row.getBoundingClientRect().height,
        metadataBelowTitle:metadata.top >= title.bottom - 1,
        metaLeft:meta.left,titleLeft:title.left,separatorLeft,
        markerRight:row.getBoundingClientRect().left + parseFloat(getComputedStyle(row, '::before').left) + parseFloat(getComputedStyle(row, '::before').width),
        rankTextLeft:rankText.getBoundingClientRect().left,
        brandLeft:document.querySelector('.brand-icon').getBoundingClientRect().left,
        header:{height:top.getBoundingClientRect().height,position:topStyle.position,
          background:topStyle.backgroundColor,navMinHeight:topNavStyle.minHeight,
          navPaddingLeft:topNavStyle.paddingLeft,navPaddingRight:topNavStyle.paddingRight,
          brandMarginLeft:brandLinkStyle.marginLeft,brandMarginRight:brandLinkStyle.marginRight,
          configFontSize:configStyle.fontSize,configPaddingLeft:configStyle.paddingLeft,
          configPaddingRight:configStyle.paddingRight},
        capacity:Math.floor((innerHeight-document.querySelector('.top').getBoundingClientRect().height)/row.getBoundingClientRect().height)};
    "#,
            vec![],
        )
        .await?;
    assert!(
        desktop_density["height"].as_f64().unwrap_or(f64::MAX) <= 48.0
            && desktop_density["capacity"].as_u64().unwrap_or_default() >= 16,
        "compact desktop rows should fit roughly 18 entries without shrinking titles: {desktop_density}"
    );
    assert_eq!(desktop_density["metadataBelowTitle"], true);
    let mut mobile_header = layout["header"].clone();
    let mut desktop_header = desktop_density["header"].clone();
    let mobile_height = mobile_header
        .as_object_mut()
        .unwrap()
        .remove("height")
        .unwrap();
    let desktop_height = desktop_header
        .as_object_mut()
        .unwrap()
        .remove("height")
        .unwrap();
    assert!(
        (mobile_height.as_f64().unwrap() - desktop_height.as_f64().unwrap()).abs() <= 1.0,
        "mobile header stays a single compact row with navigation in the bottom bar"
    );
    assert_eq!(
        desktop_header, mobile_header,
        "responsive headers should share colors, spacing, and touch targets"
    );
    assert!(
        (desktop_density["metaLeft"].as_f64().unwrap_or_default()
            - desktop_density["titleLeft"].as_f64().unwrap_or_default())
        .abs()
            <= 1.0,
        "desktop metadata should share the title text axis: {desktop_density}"
    );
    assert!(
        (desktop_density["rankTextLeft"].as_f64().unwrap_or_default()
            - desktop_density["brandLeft"].as_f64().unwrap_or_default()
            - 8.0)
            .abs()
            <= 1.0,
        "desktop rows should share the extra 8px inset used on mobile: {desktop_density}"
    );
    assert!(
        (desktop_density["markerRight"].as_f64().unwrap_or_default()
            - desktop_density["separatorLeft"]
                .as_f64()
                .unwrap_or_default())
        .abs()
            <= 1.0,
        "desktop separators should meet the right edge of the selection bar: {desktop_density}"
    );
    screenshot(client, "desktop-feed").await?;
    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":320,"height":844,"deviceScaleFactor":1,"mobile":true}),
    )
    .await?;
    client
        .execute_async("const done=arguments[arguments.length-1];document.documentElement.style.fontSize='200%';requestAnimationFrame(()=>requestAnimationFrame(()=>done(true)))", vec![])
        .await?;
    assert_eq!(
        client
            .execute(
                "return document.documentElement.scrollWidth <= document.documentElement.clientWidth+1",
                vec![]
            )
            .await?,
        true,
        "overflowing elements at 200% text: {}",
        client.execute("return {overflow:[...document.querySelectorAll('body *')].map(e=>({tag:e.tagName,cls:e.className,right:e.getBoundingClientRect().right,width:e.getBoundingClientRect().width})).filter(e=>e.right>document.documentElement.clientWidth+1).slice(0,12),layout:['html','body','.main','.rows','.row','.feed-toolbar'].map(s=>{const e=document.querySelector(s),c=getComputedStyle(e),r=e.getBoundingClientRect();return {s,left:r.left,width:r.width,pad:c.padding,margin:c.margin,font:c.fontSize}})}",vec![]).await?
    );
    client
        .execute(
            "document.documentElement.style.fontSize='';localStorage.setItem('aggr:theme','dark')",
            vec![],
        )
        .await?;
    emulate(
        client,
        "Emulation.setEmulatedMedia",
        json!({"features":[{"name":"prefers-reduced-motion","value":"reduce"}]}),
    )
    .await?;
    client.goto(&fixture.base).await?;
    assert_eq!(
        client
            .execute("return window.swup.options.native", vec![])
            .await?,
        false
    );
    screenshot(client, "mobile-dark").await?;
    client
        .find(Locator::Css("[data-row-open]"))
        .await?
        .click()
        .await?;
    wait_for(client, "!!document.querySelector('.body pre')").await?;
    client
        .execute("history.replaceState(history.state,'',location.pathname+'?reading=1#article');window.readingSentinel=42;window.scrollTo(0,300)", vec![])
        .await?;
    let reading_url = client.current_url().await?;
    fixture.deploy_app_update()?;
    client.execute_async("const done=arguments[arguments.length-1];navigator.serviceWorker.getRegistration().then(r=>r.update()).then(()=>done(true),e=>done(e.message))", vec![]).await?;
    wait_for(
        client,
        "document.documentElement.dataset.updateState === 'ready'",
    )
    .await?;
    assert_eq!(
        client
            .execute("return window.readingSentinel", vec![])
            .await?,
        42
    );
    assert_eq!(client.execute("return window.scrollY", vec![]).await?, 300);
    client
        .find(Locator::Css("#pwa-refresh"))
        .await?
        .click()
        .await?;
    wait_for(
        client,
        "document.title.includes('Updated reading room') && window.scrollY === 300",
    )
    .await?;
    assert_eq!(
        client.current_url().await?,
        reading_url,
        "updating the app must retain the article, query, hash, and reading position"
    );
    assert_eq!(
        client
            .execute("return window.readingSentinel || null", vec![])
            .await?,
        Value::Null,
        "the update button must load the newly activated version"
    );
    client.find(Locator::Css(".brand")).await?.click().await?;
    wait_for(client, "document.title.includes('Updated reading room')").await?;
    assert_eq!(
        client
            .execute("return window.readingSentinel || null", vec![])
            .await?,
        Value::Null
    );

    fixture.offline.store(true, Ordering::Relaxed);
    client.refresh().await?;
    wait_for(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3",
    )
    .await?;
    client
        .find(Locator::Css("[data-row-open]"))
        .await?
        .click()
        .await?;
    wait_for(client, "!!document.querySelector('.body pre')").await?;
    fixture.offline.store(false, Ordering::Relaxed);

    set_offline(client, true).await?;
    wait_for(
        client,
        "!navigator.onLine && !document.querySelector('#connection-status').hidden",
    )
    .await?;
    client.refresh().await?;
    wait_for(client, "!!document.querySelector('.body pre')").await?;
    wait_for(
        client,
        "document.querySelector('.progressive-image')?.naturalWidth === 640",
    )
    .await?;
    wait_for(
        client,
        "document.querySelector('#connection-status').textContent.includes('Offline')",
    )
    .await?;
    assert!(
        client
            .execute(
                "return document.querySelector('#connection-status').textContent",
                vec![]
            )
            .await?
            .as_str()
            .unwrap_or_default()
            .contains("Offline")
    );
    set_offline(client, false).await?;
    wait_for(client, "navigator.onLine").await?;
    client
        .goto(&format!("{}preferences/", fixture.base))
        .await?;
    wait_for(
        client,
        "location.pathname.endsWith('/preferences/') && !!document.querySelector('#theme-mode')",
    )
    .await?;
    client.execute("const control=document.querySelector('#offline-items');control.value='1';control.dispatchEvent(new Event('change',{bubbles:true}))", vec![]).await?;
    wait_for(client, "document.querySelector('#offline-download-status').textContent.startsWith('Available offline: 1 of 1')").await?;
    client.execute_async(r#"
      const done=arguments[arguments.length-1];
      const namespace='aggr:'+encodeURIComponent('/reader/')+':';
      Promise.all([caches.delete(namespace+'pages'),caches.delete(namespace+'images')]).then(()=>done(true));
    "#, vec![]).await?;
    set_offline(client, true).await?;
    client
        .goto(&format!(
            "{}items/example/2026-09-01-story-45/",
            fixture.base
        ))
        .await?;
    wait_for(client, "!!document.querySelector('.body pre') && document.querySelector('.progressive-image')?.naturalWidth === 640").await?;
    assert_eq!(
        client.execute("return navigator.onLine", vec![]).await?,
        false
    );
    client
        .goto(&format!("{}offline.html", fixture.base))
        .await?;
    wait_for(
        client,
        "document.querySelectorAll('#offline-articles li').length === 1",
    )
    .await?;
    assert_eq!(
        client
            .execute(
                "return document.querySelector('#offline-articles a').href.includes('story-45/')",
                vec![]
            )
            .await?,
        true
    );
    client
        .goto(&format!("{}preferences/", fixture.base))
        .await?;
    wait_for(
        client,
        "document.querySelector('#offline-items')?.value === '1'",
    )
    .await?;
    client.execute("const control=document.querySelector('#offline-items');control.value='0';control.dispatchEvent(new Event('change',{bubbles:true}))", vec![]).await?;
    wait_for(client, "document.querySelector('#offline-download-status').textContent.startsWith('Automatic downloads are off')").await?;
    let protected_count=client.execute_async(r#"
      const done=arguments[arguments.length-1];
      caches.open('aggr:'+encodeURIComponent('/reader/')+':offline-articles').then(cache=>cache.keys()).then(keys=>done(keys.length));
    "#, vec![]).await?;
    assert_eq!(
        protected_count, 0,
        "offline selection changes must reach the worker and clear protected resources"
    );
    set_offline(client, false).await?;
    Ok(())
}

async fn no_javascript_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":390,"height":844,"deviceScaleFactor":1,"mobile":true}),
    )
    .await?;
    client.goto(&fixture.base).await?;
    anyhow::ensure!(
        client.execute("return typeof window.Swup", vec![]).await? == "undefined",
        "Page scripts must remain blocked"
    );
    assert_eq!(
        client
            .execute(
                r#"
      const visible=link=>link.getBoundingClientRect().height>0;
      const links=[...document.querySelectorAll('.mobile-tabs a')];
      return {count:links.length,visible:links.every(visible),top:[...document.querySelectorAll('.nav a')].filter(visible).length,
        flat:!document.querySelector('details.site-menu'),
        search:!!document.querySelector('[data-search-open]'),
        preferences:document.querySelector('.mobile-tabs [data-route="preferences/"]').pathname,
        overflow:document.documentElement.scrollWidth>document.documentElement.clientWidth+1};
    "#,
                vec![]
            )
            .await?,
        json!({"count":4,"visible":true,"top":2,"flat":true,"search":false,"preferences":"/reader/preferences/","overflow":false})
    );
    client
        .find(Locator::Css(".mobile-tabs a[data-route='browse/']"))
        .await?
        .click()
        .await?;
    assert_eq!(client.current_url().await?.path(), "/reader/browse/");
    assert_eq!(client.execute("return ['categories','sources','tags'].every(kind=>!!document.querySelector('.browse-group-'+kind+' .browse-entry-link'))", vec![]).await?, true, "Browse exposes category, source, and tag links without JavaScript");
    client
        .find(Locator::Css(".browse-group-categories .browse-entry-link"))
        .await?
        .click()
        .await?;
    assert_eq!(client.current_url().await?.path(), "/reader/");
    assert_eq!(
        client
            .current_url()
            .await?
            .query_pairs()
            .find(|(key, _)| key == "q")
            .map(|(_, value)| value.into_owned()),
        Some("category:\"engineering\"".into()),
        "Browse category links submit the unified feed filter without JavaScript"
    );
    client
        .goto(&format!("{}sources/example/", fixture.base))
        .await?;
    assert_eq!(client.execute("const root=document.querySelector('[data-search-root]');return [root.dataset.scopeKind,root.dataset.scopeValue]", vec![]).await?, json!(["source","example"]));
    client
        .find(Locator::Css("#q"))
        .await?
        .send_keys("article")
        .await?;
    client
        .find(Locator::Css("#q"))
        .await?
        .send_keys("\u{e007}")
        .await?;
    wait_for(client, "location.pathname === '/reader/'").await?;
    assert_eq!(client.current_url().await?.path(), "/reader/");
    let submitted = client.current_url().await?;
    assert_eq!(
        submitted
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>(),
        vec![("q".to_owned(), "source:example article".to_owned())],
        "archive search carries its scope in q without separate legacy facet parameters"
    );
    client
        .goto(&format!("{}missing/two/levels/", fixture.base))
        .await?;
    let fallback = client
        .execute(
            "return {base:document.baseURI,canonical:!!document.querySelector('link[rel=canonical]'),schema:!!document.querySelector('script[type=\"application/ld+json\"]')}",
            vec![],
        )
        .await?;
    assert_eq!(fallback["base"], fixture.base);
    assert_eq!(fallback["canonical"], false);
    assert_eq!(fallback["schema"], false);
    client.goto(&fixture.base).await?;
    client
        .find(Locator::Css("#q"))
        .await?
        .send_keys("article")
        .await?;
    client
        .find(Locator::Css("#q"))
        .await?
        .send_keys("\u{e007}")
        .await?;
    wait_for(
        client,
        "location.pathname === '/reader/' && location.search === '?q=article'",
    )
    .await?;
    client.goto(&fixture.base).await?;
    let title = client.find(Locator::Css("[data-row-open]")).await?;
    anyhow::ensure!(
        title
            .attr("href")
            .await?
            .unwrap_or_default()
            .contains("items/example/"),
        "native article link"
    );
    title.click().await?;
    anyhow::ensure!(
        client
            .find(Locator::Css(".body pre code"))
            .await?
            .text()
            .await?
            .contains("$ pwd"),
        "native article content"
    );
    screenshot(client, "mobile-no-javascript").await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn rich_search_and_complete_offline_index() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::new()?;
    let archive = fixture._directory.path().join(".aggr/data");
    for (index, text) in [
        (1, "Cobalt marmalade."),
        (44, "Rare exclusion."),
        (45, "Rare exclusion."),
    ] {
        let path = archive.join(format!(
            "items/example/2026/09/2026-09-01-story-{index:02}.md"
        ));
        let body = std::fs::read_to_string(&path)?;
        std::fs::write(path, format!("{body}\n\n{text}\n"))?;
    }
    git(&archive, &["add", "items"])?;
    git(&archive, &["commit", "-qm", "fixture search vocabulary"])?;
    let body_image = std::fs::read(fixture.out.join("body.png"))?;
    fixture.build()?;
    std::fs::write(fixture.out.join("body.png"), body_image)?;
    std::fs::write(fixture.out.join("document.pdf"), fixture_pdf())?;
    let client = browser_client().await?;
    let result = rich_search_contracts(&client, &fixture).await;
    if let Err(error) = &result {
        let _ = screenshot(&client, "rich-search-failure").await;
        eprintln!("{error:#}");
        eprintln!("search state: {}", client.execute("return {url:location.href,q:document.querySelector('#q')?.value,status:document.querySelector('#search-status')?.textContent,error:document.querySelector('.search-error')?.textContent,offline:window.searchOfflineStatus,rows:document.querySelectorAll('.search-results .row').length}", vec![]).await.unwrap_or(Value::Null));
        let published: Value =
            serde_json::from_slice(&std::fs::read(fixture.out.join("search-manifest.json"))?)?;
        eprintln!(
            "published search manifest: {}",
            json!({"version":published["version"],"docs":published["docs"]})
        );
        eprintln!("browser search manifest: {}", client.execute_async("const done=arguments[arguments.length-1];fetch(new URL('search-manifest.json',document.baseURI),{cache:'no-store'}).then(r=>r.json()).then(m=>done({version:m.version,docs:m.docs})).catch(e=>done(String(e)))",vec![]).await.unwrap_or(Value::Null));
    }
    let _ = set_offline(&client, false).await;
    client.close().await?;
    result
}

async fn search_query(client: &Client, query: &str, count: usize) -> Result<()> {
    client.execute("const input=document.querySelector('#q');input.focus();input.value=arguments[0];input.setSelectionRange(input.value.length,input.value.length);input.dispatchEvent(new Event('input',{bubbles:true}));document.querySelector('#search-form').dispatchEvent(new Event('submit',{bubbles:true,cancelable:true}))", vec![json!(query)]).await?;
    wait_for(client, &format!("document.querySelector('#search-status')?.textContent === '{} {}' && !document.querySelector('.search-error')", count, if count == 1 { "article" } else { "articles" })).await?;
    anyhow::ensure!(
        client
            .execute("return document.activeElement?.id", vec![])
            .await?
            == "q",
        "search results must not steal the typing focus"
    );
    Ok(())
}

async fn rich_search_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    client.goto(&fixture.base).await?;
    client
        .execute("localStorage.setItem('aggr:feed-page-size','25')", vec![])
        .await?;
    client.refresh().await?;
    wait_for(
        client,
        "navigator.serviceWorker.controller && document.querySelector('#q')",
    )
    .await?;
    client.execute("window.searchOfflineStatus=null;navigator.serviceWorker.addEventListener('message',event=>{if(event.data?.type==='AGGR_OFFLINE_STATUS')window.searchOfflineStatus=event.data});navigator.serviceWorker.controller.postMessage({type:'AGGR_OFFLINE_GET_STATUS'})", vec![]).await?;
    wait_for(client, "window.searchOfflineStatus?.search?.phase==='ready' && window.searchOfflineStatus.saved?.length===4").await?;
    let ready = client
        .execute("return window.searchOfflineStatus.search", vec![])
        .await?;
    anyhow::ensure!(
        ready["downloadedFiles"] == ready["totalFiles"]
            && ready["downloadedBytes"] == ready["totalBytes"],
        "ready means the entire index, not only its runtime, is saved"
    );
    let complete=client.execute_async("const done=arguments[arguments.length-1];(async()=>{const m=await(await fetch('search-manifest.json')).json();const c=await caches.open('aggr:'+encodeURIComponent('/reader/')+':offline-search-'+m.version);const keys=new Set((await c.keys()).map(k=>k.url));return m.docs===45&&m.files.every(f=>keys.has(new URL(f.url,location.href).href))})().then(done)",vec![]).await?;
    anyhow::ensure!(
        complete == true,
        "every index shard must exist before readiness"
    );
    anyhow::ensure!(client.execute("return document.querySelector('#search-query-help').hidden && document.querySelector('#q').getAttribute('aria-expanded')==='false'", vec![]).await? == true, "query help and completion must start collapsed");
    let before_help = client
        .execute(
            "return document.querySelector('[data-static-feed]').getBoundingClientRect().top",
            vec![],
        )
        .await?;
    client
        .execute("document.querySelector('#q').focus()", vec![])
        .await?;
    anyhow::ensure!(
        client
            .execute(
                "return document.querySelector('#search-query-help').hidden",
                vec![]
            )
            .await?
            == true,
        "keyboard focus must not open hover help"
    );
    client.execute("document.querySelector('.search-control').dispatchEvent(new PointerEvent('pointerenter',{pointerType:'touch'}))",vec![]).await?;
    anyhow::ensure!(
        client
            .execute(
                "return document.querySelector('#search-query-help').hidden",
                vec![]
            )
            .await?
            == true,
        "touch must not open hover help"
    );
    let hover=client.execute("const r=document.querySelector('#q').getBoundingClientRect();return {x:r.left+20,y:r.top+r.height/2}",vec![]).await?;
    emulate(
        client,
        "Input.dispatchMouseEvent",
        json!({"type":"mouseMoved","x":hover["x"],"y":hover["y"]}),
    )
    .await?;
    wait_for(client,"!document.querySelector('#search-query-help').hidden && document.querySelector('#search-query-help').getAttribute('role')==='tooltip'").await?;
    anyhow::ensure!(
        client
            .execute(
                "return document.querySelector('[data-static-feed]').getBoundingClientRect().top",
                vec![]
            )
            .await?
            == before_help,
        "query help overlays the page without shifting the feed"
    );
    key(client, "Escape").await?;
    wait_for(client,"document.querySelector('#search-query-help').hidden && document.querySelector('#q').getAttribute('aria-expanded')==='false'").await?;

    // The first query occurs after going offline: no query or result fragment was warmed online.
    fixture.offline.store(true, Ordering::Relaxed);
    set_offline(client, true).await?;
    search_query(client, "\"cobalt marmalade\"", 1).await?;
    wait_for(
        client,
        "document.querySelector('.search-results [data-saved-offline=false]')",
    )
    .await?;
    anyhow::ensure!(
        client
            .execute(
                "return document.querySelector('.search-results [data-row-open]').textContent",
                vec![]
            )
            .await?
            == "Article 1",
        "fresh offline full-text search must find an old, unsaved article"
    );
    search_query(client, "\"rare exclusion\"", 2).await?;
    wait_for(
        client,
        "document.querySelectorAll('.search-results [data-saved-offline=true]').length===2",
    )
    .await?;
    fixture.offline.store(false, Ordering::Relaxed);
    set_offline(client, false).await?;

    search_query(
        client,
        "source:example category:engineering tag:rust date:>=2026-09-02",
        22,
    )
    .await?;
    search_query(client, "comfortably \"current page\" -\"rare exclusion\" source:example category:engineering tag:reading", 43).await?;
    anyhow::ensure!(
        client
            .execute(
                "return document.querySelectorAll('.search-results .row').length",
                vec![]
            )
            .await?
            == 25,
        "the total must count all compound matches before pagination"
    );
    client
        .find(Locator::Css(
            "[data-search-results] .pager button:last-child",
        ))
        .await?
        .click()
        .await?;
    wait_for(client, "document.querySelectorAll('.search-results .row').length===18 && document.querySelector('.search-results .rank')?.textContent==='26.'").await?;
    anyhow::ensure!(
        client
            .execute(
                "return document.querySelector('#search-status').textContent",
                vec![]
            )
            .await?
            == "43 articles",
        "page changes must preserve the complete count"
    );

    client.execute("const q=document.querySelector('#q');q.focus();q.value='source:ex tag:rust';q.setSelectionRange(9,9);q.dispatchEvent(new Event('input',{bubbles:true}));q.dispatchEvent(new Event('select',{bubbles:true}))",vec![]).await?;
    wait_for(client,"Array.from(document.querySelectorAll('.search-completion')).some(e=>e.textContent.includes('Example'))").await?;
    client
        .active_element()
        .await?
        .send_keys("\u{e015}\u{e007}")
        .await?;
    wait_for(client,"document.querySelector('#q').value.includes('source:example') && document.querySelector('#q').value.includes('tag:rust')").await?;
    anyhow::ensure!(
        client
            .execute("return document.activeElement?.id", vec![])
            .await?
            == "q",
        "completion retains editor focus"
    );
    wait_for(client,"!document.querySelector('.search-completions') && document.querySelector('#q').getAttribute('aria-expanded')==='false'").await?;
    client.execute("const q=document.querySelector('#q');q.value='sort:';q.setSelectionRange(5,5);q.dispatchEvent(new Event('input',{bubbles:true}))",vec![]).await?;
    wait_for(
        client,
        "document.querySelectorAll('.search-completion').length===3",
    )
    .await?;
    client.active_element().await?.send_keys("\u{e015}").await?;
    let first_choice=client.execute("return document.querySelector('.search-completion[data-selected] .completion-label')?.textContent",vec![]).await?;
    client.active_element().await?.send_keys("\u{e015}").await?;
    let second_choice=client.execute("return document.querySelector('.search-completion[data-selected] .completion-label')?.textContent",vec![]).await?;
    anyhow::ensure!(
        first_choice.is_string() && second_choice.is_string() && first_choice != second_choice,
        "successive arrows must advance selection without keyup resetting it"
    );
    key(client, "Enter").await?;
    wait_for(client,&format!("document.querySelector('#q').value.trim()==={} && !document.querySelector('.search-completions')",json!(format!("sort:{}",second_choice.as_str().unwrap_or_default())))).await?;
    wait_for(
        client,
        "document.querySelector('#search-status')?.textContent==='45 articles'",
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(250)).await;
    anyhow::ensure!(client.execute("return !document.querySelector('.search-completions') && document.querySelector('#q').getAttribute('aria-expanded')==='false'",vec![]).await?==true,"selected completion must stay closed when results arrive");

    // Composition updates must not run or publish a half-composed query.
    client.execute("window.beforeCompositionURL=location.href;const q=document.querySelector('#q');q.dispatchEvent(new CompositionEvent('compositionstart',{bubbles:true}));q.value='cobalt';q.dispatchEvent(new InputEvent('input',{bubbles:true,isComposing:true}));q.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true,isComposing:true}))",vec![]).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    anyhow::ensure!(
        client
            .execute("return location.href===window.beforeCompositionURL", vec![])
            .await?
            == true,
        "IME composition must not submit partial text"
    );
    client.execute("document.querySelector('#q').dispatchEvent(new CompositionEvent('compositionend',{bubbles:true,data:'cobalt'}))",vec![]).await?;
    wait_for(
        client,
        "document.querySelector('#search-status')?.textContent==='1 article'",
    )
    .await?;

    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":390,"height":844,"deviceScaleFactor":1,"mobile":true}),
    )
    .await?;
    emulate(
        client,
        "Emulation.setTouchEmulationEnabled",
        json!({"enabled":true,"maxTouchPoints":1}),
    )
    .await?;
    client.execute("const q=document.querySelector('#q');q.focus();q.value='category:eng';q.setSelectionRange(q.value.length,q.value.length);q.dispatchEvent(new Event('input',{bubbles:true}))",vec![]).await?;
    wait_for(
        client,
        "document.querySelector('.search-completion')?.textContent.toLowerCase().includes('engineering')",
    )
    .await?;
    let point=client.execute("const r=document.querySelector('.search-completion').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}",vec![]).await?;
    emulate(
        client,
        "Input.dispatchTouchEvent",
        json!({"type":"touchStart","touchPoints":[{"x":point["x"],"y":point["y"]}]}),
    )
    .await?;
    emulate(
        client,
        "Input.dispatchTouchEvent",
        json!({"type":"touchEnd","touchPoints":[]}),
    )
    .await?;
    wait_for(
        client,
        "document.querySelector('#q').value.trim()==='category:engineering'",
    )
    .await?;
    wait_for(client,"!document.querySelector('.search-completions') && document.querySelector('#q').getAttribute('aria-expanded')==='false'").await?;

    for (directory, scope) in [
        ("sources/example/", "source:example"),
        ("categories/engineering/", "category:engineering"),
        ("tags/rust/", "tag:rust"),
    ] {
        client.goto(&format!("{}{directory}", fixture.base)).await?;
        wait_for(
            client,
            &format!("document.querySelector('#q')?.value==='{}'", scope),
        )
        .await?;
        anyhow::ensure!(
            client.current_url().await?.query().is_none(),
            "archive scope must not rewrite its unsearched route"
        );
        client
            .find(Locator::Css(".search-clear"))
            .await?
            .click()
            .await?;
        wait_for(
            client,
            "location.pathname==='/reader/' && document.querySelector('#q')?.value===''",
        )
        .await?;
    }

    for (directory, title, introduction, qualifier) in [
        (
            "categories/",
            "Categories",
            "Categories group sources by subject.",
            "category:engineering",
        ),
        (
            "sources/",
            "Sources",
            "Sources are the feeds and sites you follow.",
            "source:example",
        ),
        (
            "tags/",
            "Tags",
            "Tags describe topics attached to articles.",
            "tag:reading",
        ),
    ] {
        client.goto(&format!("{}{directory}", fixture.base)).await?;
        wait_for(client, "document.querySelector('.browse-entry-link')").await?;
        let headings = client.execute("return [...document.querySelectorAll('.browse-page h1,.browse-page h2')].map(e=>e.textContent.trim())", vec![]).await?;
        anyhow::ensure!(
            headings == json!([title]),
            "{directory} must have one heading without repeating the directory name: {headings}"
        );
        anyhow::ensure!(
            client
                .execute(
                    "return document.querySelector('.browse-intro').textContent",
                    vec![]
                )
                .await?
                .as_str()
                .is_some_and(|text| text.contains(introduction)),
            "{directory} explains its own organizing concept"
        );
        canonical_facet_click(client, ".browse-entry-link", qualifier).await?;
    }
    for (selector, qualifier) in [
        (".row .meta .domain", "source:example"),
        (".row .meta .category a", "category:engineering"),
    ] {
        client.goto(&fixture.base).await?;
        wait_for(
            client,
            &format!("document.querySelector({})", json!(selector)),
        )
        .await?;
        canonical_facet_click(client, selector, qualifier).await?;
    }
    client
        .goto(&format!(
            "{}items/example/2026-09-01-story-45/",
            fixture.base
        ))
        .await?;
    wait_for(client, "document.querySelector('.item-tags .tag')").await?;
    canonical_facet_click(client, ".item-tags .tag", "tag:reading").await?;
    canonical_facet_click(client, ".search-results .meta .domain", "source:example").await?;
    canonical_facet_click(
        client,
        ".search-results .meta .category a",
        "category:engineering",
    )
    .await?;

    let preview_article = walkdir::WalkDir::new(fixture.out.join("items"))
        .into_iter()
        .filter_map(Result::ok)
        .find(|entry| {
            entry.file_type().is_file()
                && entry.file_name() == "index.html"
                && std::fs::read_to_string(entry.path())
                    .ok()
                    .and_then(|html| {
                        html.split_once("<footer class=\"article-footer\"")
                            .map(|(_, footer)| {
                                footer.contains("class=\"preview-image\"")
                                    && footer.matches("class=\"article-more-card\"").count() == 2
                            })
                    })
                    .unwrap_or(false)
        })
        .context("fixture article recommendation with an archived thumbnail")?;
    let relative = preview_article
        .path()
        .strip_prefix(&fixture.out)?
        .parent()
        .context("article directory")?
        .to_string_lossy();
    client.goto(&format!("{}{relative}/", fixture.base)).await?;
    wait_for(
        client,
        "document.querySelector('.article-more-card .preview-image')",
    )
    .await?;
    anyhow::ensure!(client.execute("const image=document.querySelector('.article-more-card .preview-image');const card=image.closest('.article-more-card');return image.width>0 && image.height>0 && image.getAttribute('loading')==='lazy' && ['.title','.meta .domain','.meta .category a','.meta .published-date','.meta .reading-stats','.meta .u-bookmark-of'].every(selector=>card.querySelector(selector))",vec![]).await?==true,"recommendation cards retain reserved thumbnails and full feed metadata");
    for width in [1280, 390] {
        emulate(
            client,
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":width<600}),
        )
        .await?;
        let layout=client.execute("const cards=[...document.querySelectorAll('.article-more-card')].map(card=>card.getBoundingClientRect());return {count:cards.length,sameRow:Math.abs(cards[0].top-cards[1].top)<=1,separateColumns:cards[1].left>=cards[0].right,stacked:cards[1].top>=cards[0].bottom,overflow:document.documentElement.scrollWidth>document.documentElement.clientWidth+1}",vec![]).await?;
        anyhow::ensure!(
            layout["count"] == 2 && layout["overflow"] == false,
            "recommendation cards fit the viewport at {width}px: {layout}"
        );
        anyhow::ensure!(
            layout["stacked"] == true,
            "recommendations stack vertically at {width}px: {layout}"
        );
    }

    client
        .goto(&format!("{}preferences/", fixture.base))
        .await?;
    wait_for(client, "document.querySelector('#offline-items')").await?;
    client.execute("window.searchOfflineStatus=null;navigator.serviceWorker.addEventListener('message',e=>{if(e.data?.type==='AGGR_OFFLINE_STATUS')window.searchOfflineStatus=e.data});const input=document.querySelector('#offline-items');input.value='1';input.dispatchEvent(new Event('change',{bubbles:true}))",vec![]).await?;
    wait_for(client,"window.searchOfflineStatus?.saved?.length===1 && window.searchOfflineStatus?.search?.phase==='ready'").await?;
    anyhow::ensure!(
        client
            .execute(
                "return window.searchOfflineStatus.search.activeVersion",
                vec![]
            )
            .await?
            == ready["activeVersion"],
        "changing article count keeps the same complete search index"
    );
    // A deployment can expose a new online index before its worker/index replacement succeeds.
    std::fs::write(
        fixture._directory.path().join(".block-worker-updates"),
        b"503",
    )?;
    fixture.deploy_content_update(46)?;
    client
        .goto(&format!(
            "{}?q=%22Newly%20delivered%20reading%22",
            fixture.base
        ))
        .await?;
    wait_for(
        client,
        "document.querySelector('#search-status')?.textContent==='1 article'",
    )
    .await?;
    anyhow::ensure!(
        client
            .execute(
                "return document.querySelector('.search-results [data-row-open]').textContent",
                vec![]
            )
            .await?
            == "Freshly delivered article 46",
        "the online reader must have loaded the new deployment's index"
    );
    client.execute("window.searchOfflineStatus=null;navigator.serviceWorker.addEventListener('message',e=>{if(e.data?.type==='AGGR_OFFLINE_STATUS')window.searchOfflineStatus=e.data});navigator.serviceWorker.controller.postMessage({type:'AGGR_OFFLINE_GET_STATUS'})",vec![]).await?;
    wait_for(
        client,
        "window.searchOfflineStatus?.search?.phase==='ready'",
    )
    .await?;
    anyhow::ensure!(
        client
            .execute(
                "return window.searchOfflineStatus.search.activeVersion",
                vec![]
            )
            .await?
            == ready["activeVersion"],
        "the complete old index remains committed while the online reader loads the new one"
    );
    fixture.offline.store(true, Ordering::Relaxed);
    set_offline(client, true).await?;
    wait_for(
        client,
        "document.querySelector('#search-status')?.textContent==='0 articles'",
    )
    .await?;
    search_query(client, "\"cobalt marmalade\"", 1).await?;
    anyhow::ensure!(client.execute("return document.querySelector('.search-results [data-saved-offline=false]')!==null",vec![]).await?==true,"falling back to the older complete index does not mark uncached articles saved");
    client
        .goto(&format!("{}preferences/", fixture.base))
        .await?;
    wait_for(client, "document.querySelector('#offline-items')").await?;
    client.execute("window.searchOfflineStatus=null;navigator.serviceWorker.addEventListener('message',e=>{if(e.data?.type==='AGGR_OFFLINE_STATUS')window.searchOfflineStatus=e.data});navigator.serviceWorker.controller.postMessage({type:'AGGR_OFFLINE_GET_STATUS'})",vec![]).await?;
    wait_for(
        client,
        "window.searchOfflineStatus?.search?.phase==='ready'",
    )
    .await?;
    anyhow::ensure!(
        client
            .execute(
                "return window.searchOfflineStatus.search.activeVersion",
                vec![]
            )
            .await?
            == ready["activeVersion"],
        "the online-to-offline transition selects the previously committed index"
    );
    client.execute("window.zeroStatusRegressions=[];window.sawDisabledStatus=false;navigator.serviceWorker.addEventListener('message',event=>{const status=event.data;if(status?.type!=='AGGR_OFFLINE_STATUS')return;if((status.requested===0 && status.search?.phase!=='disabled') || (window.sawDisabledStatus && status.requested!==0))window.zeroStatusRegressions.push(status);if(status.requested===0 && status.search?.phase==='disabled')window.sawDisabledStatus=true});const worker=navigator.serviceWorker.controller;for(let i=0;i<8;i++)worker.postMessage({type:'AGGR_OFFLINE_GET_STATUS'});const input=document.querySelector('#offline-items');input.value='0';input.dispatchEvent(new Event('change',{bubbles:true}));for(let i=0;i<8;i++)worker.postMessage({type:'AGGR_OFFLINE_GET_STATUS'})",vec![]).await?;
    wait_for(client,"window.searchOfflineStatus?.requested===0 && window.searchOfflineStatus?.search?.phase==='disabled'").await?;
    let empty=client.execute_async("const done=arguments[arguments.length-1];caches.keys().then(keys=>done(!keys.some(k=>k.includes(':offline-search-'))))",vec![]).await?;
    anyhow::ensure!(
        empty == true,
        "disabling offline saving removes every protected index generation"
    );
    client
        .execute_async(
            "const done=arguments[arguments.length-1];setTimeout(done,250)",
            vec![],
        )
        .await?;
    anyhow::ensure!(
        client
            .execute("return window.zeroStatusRegressions", vec![])
            .await?
            == json!([]),
        "concurrent read-only status replies must not resurrect ready search after N=0"
    );
    Ok(())
}

async fn canonical_facet_click(client: &Client, selector: &str, qualifier: &str) -> Result<()> {
    let (kind, value) = qualifier
        .split_once(':')
        .context("fixture facet qualifier")?;
    let qualifier = format!("{kind}:{}", json!(value));
    let url = client
        .execute(
            "return document.querySelector(arguments[0]).href",
            vec![json!(selector)],
        )
        .await?;
    let url = url::Url::parse(url.as_str().context("facet link URL")?)?;
    anyhow::ensure!(
        url.path() == "/reader/"
            && url
                .query_pairs()
                .any(|(key, value)| key == "q" && value == qualifier),
        "facet href must be a copyable canonical feed query: {url}"
    );
    client.find(Locator::Css(selector)).await?.click().await?;
    wait_for(client, &format!("location.pathname==='/reader/' && new URL(location.href).searchParams.get('q')==={} && document.querySelector('#q')?.value==={} && document.querySelector('#search-status')?.textContent==='45 articles'",json!(qualifier),json!(qualifier))).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn svelte_components_preserve_reader_lifecycles() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    for base_path in ["", "reader/"] {
        let fixture = Fixture::with_base_path(false, base_path)?;
        let archive = fixture._directory.path().join(".aggr/data");
        let path = archive.join("items/example/2026/09/2026-09-01-story-36.md");
        let cover = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            256,
            256,
            image::Rgb([192, 116, 72]),
        ));
        let mut cover_bytes = std::io::Cursor::new(Vec::new());
        cover.write_to(&mut cover_bytes, image::ImageFormat::Jpeg)?;
        let cover_bytes = cover_bytes.into_inner();
        let cover_hash = hex::encode(Sha1::digest(&cover_bytes));
        let cover_name = format!("2026-09-01-story-36.preview-{}.jpg", &cover_hash[..12]);
        std::fs::write(
            path.parent()
                .context("audio fixture directory")?
                .join(&cover_name),
            cover_bytes,
        )?;
        let text = std::fs::read_to_string(&path)?.replace(
            "content: feed\n",
            &format!(
                "content: feed\npreview:\n  file: {cover_name}\n  width: 256\n  height: 256\nextra:\n  duration_seconds: 600\n  audio_url: {}media.wav\n",
                fixture.base
            ),
        );
        std::fs::write(&path, text)?;
        git(&archive, &["add", "."])?;
        git(&archive, &["commit", "-qm", "fixture component audio"])?;
        fixture.build()?;
        let samples = 8000_u32;
        let mut wave = b"RIFF".to_vec();
        wave.extend((samples + 36).to_le_bytes());
        wave.extend(b"WAVEfmt ");
        wave.extend(16_u32.to_le_bytes());
        wave.extend(1_u16.to_le_bytes());
        wave.extend(1_u16.to_le_bytes());
        wave.extend(8000_u32.to_le_bytes());
        wave.extend(8000_u32.to_le_bytes());
        wave.extend(1_u16.to_le_bytes());
        wave.extend(8_u16.to_le_bytes());
        wave.extend(b"data");
        wave.extend(samples.to_le_bytes());
        wave.resize(wave.len() + samples as usize, 128);
        std::fs::write(fixture.out.join("media.wav"), wave)?;
        let client = browser_client().await?;
        let result = async {
                let mobile = !base_path.is_empty();
                emulate(&client, "Emulation.setDeviceMetricsOverride", json!({"width":if mobile {390}else{1280},"height":844,"deviceScaleFactor":1,"mobile":mobile})).await?;
                client.goto(&fixture.base).await?;
                wait_for(&client, "typeof window.swup?.navigate === 'function' && document.querySelector('.search-command')").await?;
                client.execute("window.swup.navigate(arguments[0])", vec![json!(format!("{}preferences/", fixture.base))]).await?;
                wait_for(&client, "!window.swup.navigating && document.querySelector('#theme-mode')").await?;
                let report = client
                    .execute_async(
                        include_str!("fixtures/component_lifecycle_checks.js"),
                        vec![json!(fixture.base)],
                    )
                    .await?;
                anyhow::ensure!(
                    report.get("error").is_none(),
                    "component lifecycle ({base_path}): {report}"
                );
                anyhow::ensure!(
                    report["cycles"] == 3,
                    "all navigation cycles completed: {report}"
                );
                client.goto(&format!("{}items/example/2026-09-01-story-36/", fixture.base)).await?;
                wait_for(&client, "!!document.querySelector('.native-audio.is-enhanced')").await?;
                screenshot(&client, if mobile {"podcast-player-mobile"} else {"podcast-player-desktop"}).await?;
                Ok(())
            }
            .await;
        if result.is_err() {
            let _ = screenshot(&client, "component-lifecycle-failure").await;
        }
        client.close().await?;
        result?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn reader_client_performance_metrics() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let mut measurements = Vec::new();
    for (environment, width, mobile, rate) in
        [("desktop", 1280, false, 1), ("mobile-4x", 390, true, 4)]
    {
        for run in 0..5 {
            let client = browser_client().await?;
            let measurement: Result<Value> = async {
                        emulate(&client, "Network.enable", json!({})).await?;
                        emulate(&client, "Network.setCacheDisabled", json!({"cacheDisabled": true})).await?;
                        emulate(&client, "Network.setBlockedURLs", json!({"urls": ["*.png", "*.jpg", "*.jpeg", "*.webp", "*.gif", "*.avif"]})).await?;
                        emulate(&client, "Emulation.setDeviceMetricsOverride", json!({"width": width, "height": 844, "deviceScaleFactor": 1, "mobile": mobile})).await?;
                        emulate(&client, "Emulation.setCPUThrottlingRate", json!({"rate": rate})).await?;
                        emulate(&client, "Page.addScriptToEvaluateOnNewDocument", json!({"source": "window.__aggrPerfErrors=[];addEventListener('error',e=>window.__aggrPerfErrors.push(e.message));addEventListener('unhandledrejection',e=>window.__aggrPerfErrors.push(String(e.reason)));requestAnimationFrame(function ready(){if(typeof window.swup?.navigate==='function'&&document.querySelector('.search-command'))window.__aggrPerfReadyAt=performance.now();else requestAnimationFrame(ready)})"})).await?;
                        client.goto(&fixture.base).await?;
                        let value = client.execute_async(r#"
                          const base=arguments[0],done=arguments[arguments.length-1];
                          (async()=>{
                            const until=async check=>{const started=performance.now();while(!check()){if(performance.now()-started>15000)throw Error('timed out waiting for performance scenario');await new Promise(requestAnimationFrame)}};
                            await until(()=>window.__aggrPerfReadyAt&&performance.getEntriesByName('first-contentful-paint').length);
                            const initialReadyMs=window.__aggrPerfReadyAt,fcpMs=performance.getEntriesByName('first-contentful-paint')[0].startTime;
                            let started=performance.now();
                            await window.swup.navigate(new URL('preferences/',base).href);
                            await until(()=>!window.swup.navigating&&document.querySelector('#theme-mode'));
                            const preferencesNavigationMs=performance.now()-started;
                            started=performance.now();
                            await window.swup.navigate(base);
                            await until(()=>!window.swup.navigating&&document.querySelector('.search-command'));
                            const feedNavigationMs=performance.now()-started;
                            const input=document.querySelector('#q');
                            started=performance.now();
                            input.value='category:engineering "Reading comfortably"';
                            input.setSelectionRange(input.value.length,input.value.length);
                            input.dispatchEvent(new Event('input',{bubbles:true}));
                            await until(()=>document.querySelector('.search-results .row'));
                            const firstSearchMs=performance.now()-started;
                            done({initialReadyMs,fcpMs,preferencesNavigationMs,feedNavigationMs,firstSearchMs,results:document.querySelectorAll('.search-results .row').length});
                          })().catch(error=>done({error:error.stack||String(error),ready:window.__aggrPerfReadyAt,swup:!!window.swup,search:!!document.querySelector('.search-command'),paint:performance.getEntriesByType('paint').map(e=>({name:e.name,start:e.startTime})),clientErrors:window.__aggrPerfErrors,url:location.href}));
                        "#, vec![json!(fixture.base)]).await?;
                        anyhow::ensure!(value.get("error").is_none(), "performance scenario ({environment}/{run}): {value}");
                        Ok(value)

            }.await;
            if measurement.is_err() {
                let _ = screenshot(&client, "client-performance-failure").await;
            }
            client.close().await?;
            let mut measurement = measurement?;
            measurement["environment"] = json!(environment);
            measurement["run"] = json!(run);
            measurements.push(measurement);
        }
    }
    let mut medians = Vec::new();
    for environment in ["desktop", "mobile-4x"] {
        let mut median = json!({"environment": environment});
        for metric in [
            "initialReadyMs",
            "fcpMs",
            "preferencesNavigationMs",
            "feedNavigationMs",
            "firstSearchMs",
        ] {
            let mut values: Vec<f64> = measurements
                .iter()
                .filter(|sample| sample["environment"] == environment)
                .filter_map(|sample| sample[metric].as_f64())
                .collect();
            values.sort_by(f64::total_cmp);
            median[metric] = json!(values[values.len() / 2]);
        }
        eprintln!("client performance median: {median}");
        medians.push(median);
    }
    let report = json!({"units": "milliseconds", "archiveItems": 45, "runsPerEnvironment": 5,
        "scope": "Current production client in fresh browser sessions, HTTP cache disabled and image requests blocked. Mobile emulates a 390px viewport with 4x CPU throttling. Local network, no pass thresholds or historical comparison.",
        "medians": medians, "measurements": measurements});
    let artifacts = Path::new("target/browser-artifacts");
    std::fs::create_dir_all(artifacts)?;
    std::fs::write(
        artifacts.join("client-performance.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn clearing_updated_search_reveals_the_latest_static_feed() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        accelerate_update_polling(&client).await?;
        emulate(&client,"Page.addScriptToEvaluateOnNewDocument",json!({"source": "(()=>{window.searchRequests=[];const originalFetch=window.fetch;window.fetch=async function(...args){try{const response=await originalFetch.apply(this,args);window.searchRequests.push({url:String(args[0]),status:response.status});return response}catch(error){window.searchRequests.push({url:String(args[0]),error:String(error)});throw error}}})()"})).await?;
        client.goto(&fixture.base).await?;
        wait_for(&client, "typeof window.swup?.navigate === 'function' && document.querySelector('.search-command')").await?;
        client.execute(r#"
          window.searchDeploymentSentinel={};
          window.originalSearchDeploymentSentinel=window.searchDeploymentSentinel;
          const input=document.querySelector('#q');
          input.value='category:engineering';
          input.setSelectionRange(input.value.length,input.value.length);
          input.dispatchEvent(new Event('input',{bubbles:true}));
        "#,vec![]).await?;
        wait_for(&client,"document.querySelector('#search-status')?.textContent.trim()==='45 articles'").await?;
        fixture.deploy_content_update(49)?;
        wait_for(&client,"document.querySelector('#search-status')?.textContent.trim()==='46 articles' && document.querySelector('.search-results [data-row-open]')?.textContent.includes('Freshly delivered article 49')").await?;
        anyhow::ensure!(client.execute("return document.querySelector('#q').value==='category:engineering' && document.querySelector('[data-static-feed]').hidden",vec![]).await?==true,"the active search survives content delivery and keeps the static feed hidden");
        client.find(Locator::Css(".search-clear")).await?.click().await?;
        wait_for(&client,"!document.querySelector('[data-static-feed]').hidden && document.querySelector('[data-static-feed] [data-row-open]')?.textContent.includes('Freshly delivered article 49')").await?;
        anyhow::ensure!(client.execute(r#"
          return !!window.searchDeploymentSentinel && window.searchDeploymentSentinel===window.originalSearchDeploymentSentinel
            && !new URL(location.href).searchParams.has('q')
            && document.querySelector('#q').value===''
            && document.activeElement===document.querySelector('#q')
            && document.querySelectorAll('.search-command').length===1
            && document.querySelector('#pwa-refresh').hidden;
        "#,vec![]).await?==true,"clearing refreshed search shows current static content without reload, duplicate controls, lost input focus, or an app-update prompt");
        Ok(())
    }.await;
    if result.is_err() {
        let _ = screenshot(&client, "search-deployment-clear-failure").await;
        eprintln!("search deployment diagnostics: {}",client.execute("return {error:document.querySelector('.search-error')?.textContent,requests:window.searchRequests,base:document.baseURI}",vec![]).await.unwrap_or(Value::Null));
    }
    client.close().await?;
    result
}
