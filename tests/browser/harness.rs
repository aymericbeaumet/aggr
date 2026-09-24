//! Shared browser harness: the generated fixture site and its HTTP server, WebDriver session
//! helpers, self-diagnosing waits and session teardown.

// The shared integration-test helpers (git and environment isolation). Declared here rather than
// in the target root so that both the `browser` and `browser_performance` targets, which include
// this file, get the same policy.
#[path = "../support/mod.rs"]
mod support;

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use fantoccini::{Client, ClientBuilder};
use futures_util::FutureExt as _;
use serde_json::{Value, json};
use sha1::{Digest as _, Sha1};

use support::aggr_command;
pub(crate) use support::git;

pub(crate) struct Fixture {
    pub(crate) directory: tempfile::TempDir,
    pub(crate) out: PathBuf,
    pub(crate) base: String,
    pub(crate) offline: Arc<AtomicBool>,
    pub(crate) scripts_blocked: Arc<AtomicBool>,
    pub(crate) media: Arc<MediaResponses>,
    stopped: Arc<AtomicBool>,
}

#[derive(Default)]
pub(crate) struct MediaResponses {
    pub(crate) blocked: AtomicBool,
    pub(crate) app_blocked: AtomicBool,
    pub(crate) failed: AtomicBool,
    pub(crate) completed: AtomicUsize,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}

impl Fixture {
    pub(crate) fn deploy_content_update(&self, index: u32) -> Result<()> {
        let archive = self.directory.path().join(".aggr/data");
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

    pub(crate) fn build(&self) -> Result<()> {
        let root = self.directory.path();
        let output = aggr_command(root)
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
        Ok(())
    }

    pub(crate) fn new() -> Result<Self> {
        Self::with_pwa(true)
    }

    pub(crate) fn with_pwa(pwa: bool) -> Result<Self> {
        Self::with_base_path(pwa, "reader/")
    }

    pub(crate) fn with_base_path(pwa: bool, base_path: &str) -> Result<Self> {
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
            // One article carries a section, so a heading anchor has somewhere to point.
            let section = if index == 40 {
                "## A section worth linking\n\nProse beneath the section heading.\n\n"
            } else {
                ""
            };
            let markdown = format!(
                "---\ntitle: {title}\nlink: {link}\nsource: example\npublished: {published}\nupdated: {updated}\nfirst_seen: {published}\ncontent: feed\nlabels: [reading, rust]\n{preview}{archived_image}---\n\n* * *\n\nA paragraph with [first link](https://example.invalid/one) and more prose before [a comparison grid](https://example.invalid/two) continues naturally.\n\n```bash\n$ z dotfiles\n$ pwd\n/private/dotfiles\n```\n\n![An article illustration]({base}body.png)\n\nThis entry explores archive topic {index}.\n\n{section}{}\n",
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
        let output = aggr_command(root)
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
                        // Chrome keeps preconnected sockets around: the flags must be sampled per
                        // request, never when the connection is accepted.
                        let offline = is_offline.clone();
                        let scripts_blocked = block_scripts.clone();
                        let media = media_responses.clone();
                        let stopped = is_stopped.clone();
                        std::thread::spawn(move || {
                            if let Err(error) =
                                serve(stream, &root, &offline, &scripts_blocked, &media, &stopped)
                                && !is_expected_socket_error(&error)
                            {
                                eprintln!("fixture server: {error:#}");
                            }
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    // A transient accept failure must not silently retire the whole fixture
                    // site; the next request would only surface as "Failed to fetch".
                    Err(error) => {
                        eprintln!("fixture server accept: {error}");
                        std::thread::sleep(Duration::from_millis(10))
                    }
                }
            }
        });
        Ok(Self {
            directory,
            out,
            base,
            offline,
            scripts_blocked,
            media,
            stopped,
        })
    }
}

pub(crate) fn fixture_pdf() -> Vec<u8> {
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

/// Socket errors that only mean the browser hung up first; anything else is worth a line of log.
fn is_expected_socket_error(error: &anyhow::Error) -> bool {
    use std::io::ErrorKind;
    error.downcast_ref::<std::io::Error>().is_some_and(|error| {
        matches!(
            error.kind(),
            ErrorKind::BrokenPipe
                | ErrorKind::ConnectionReset
                | ErrorKind::WouldBlock
                | ErrorKind::TimedOut
        )
    })
}

#[test]
fn only_browser_hangups_are_expected_socket_errors() {
    use std::io::{Error, ErrorKind};
    assert!(is_expected_socket_error(
        &Error::from(ErrorKind::BrokenPipe).into()
    ));
    assert!(is_expected_socket_error(
        &Error::from(ErrorKind::ConnectionReset).into()
    ));
    assert!(!is_expected_socket_error(
        &Error::from(ErrorKind::PermissionDenied).into()
    ));
    assert!(!is_expected_socket_error(&anyhow::anyhow!(
        "fixture build failed"
    )));
}

#[test]
fn artifact_names_are_bounded_file_safe_slugs() {
    let name = artifact_name(
        "search::rich_search_and_complete_offline_index",
        "document.querySelector('#q')?.value.trim()==='sort:oldest' && !document.querySelector('.search-completions:not([hidden])')",
    );
    assert!(
        name.starts_with("search-rich_search_and_complete_offline_index-document-queryselector-q-value-trim-sort-oldest"),
        "{name}"
    );
    assert!(
        name.chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_')),
        "{name}"
    );
    assert!(
        name.len() <= "search-rich_search_and_complete_offline_index-".len() + 48,
        "{name}"
    );
    assert!(!name.ends_with('-'), "{name}");
    assert_eq!(artifact_name("browser", "!!x"), "browser-x");
}

fn serve(
    mut stream: TcpStream,
    root: &Path,
    offline: &AtomicBool,
    scripts_blocked: &AtomicBool,
    media: &MediaResponses,
    stopped: &AtomicBool,
) -> Result<()> {
    // BSD sockets (macOS) inherit the listener's non-blocking mode: without this the first read
    // can return WouldBlock before the request bytes arrive and the connection is dropped
    // unanswered, which the browser reports as "Failed to fetch".
    stream.set_nonblocking(false)?;
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
    // Sampled once the request line is in hand, like media/stopped, so a flag flipped by the test
    // applies to the next request even on a socket Chrome accepted earlier.
    let offline = offline.load(Ordering::Relaxed);
    let scripts_blocked = scripts_blocked.load(Ordering::Relaxed);
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

/// The suite runs every browser test in parallel; a loaded runner can stall a mount well past
/// the time the same step takes in isolation, so the budget is tunable per machine.
fn wait_timeout() -> Duration {
    Duration::from_secs(
        std::env::var("AGGR_BROWSER_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(45),
    )
}

/// A file-name stem for timeout artifacts: the running test plus a hint of the expression.
fn artifact_name(test: &str, expression: &str) -> String {
    let test = test.replace("::", "-");
    let slug: String = expression
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug: String = slug.chars().take(48).collect();
    format!("{test}-{}", slug.trim_end_matches('-'))
}

/// A snapshot of the page that explains most stalls without reopening the browser.
pub(crate) async fn page_diagnostics(client: &Client) -> Value {
    client
        .execute(
            "return {url:location.href,readyState:document.readyState,head:document.head.outerHTML.slice(0,4000),scripts:[...document.scripts].map(s=>s.src),errors:window.__aggrErrors||[],activeElement:document.activeElement?.outerHTML?.slice(0,300),q:document.querySelector('#q')?.value,searchStatus:document.querySelector('#search-status')?.textContent,rows:document.querySelectorAll('#list .row').length}",
            vec![],
        )
        .await
        .unwrap_or(Value::Null)
}

pub(crate) async fn wait_for(client: &Client, expression: &str) -> Result<()> {
    let start = Instant::now();
    let timeout = wait_timeout();
    loop {
        // A wait is a question about a page that is still changing: an expression that reaches
        // through something not there yet is simply not true yet, and saying so leaves the
        // timeout to report where it got stuck.
        if client
            .execute(
                &format!("try {{ return Boolean({expression}) }} catch (error) {{ return false }}"),
                vec![],
            )
            .await?
            == json!(true)
        {
            return Ok(());
        }
        if start.elapsed() > timeout {
            let thread = std::thread::current();
            let name = format!(
                "{}-timeout",
                artifact_name(thread.name().unwrap_or("browser"), expression)
            );
            let _ = screenshot(client, &name).await;
            let diagnostics = page_diagnostics(client).await;
            let directory = Path::new("target/browser-artifacts");
            // The file name only carries a slug of the predicate; the full text goes with the
            // snapshot so the artifact explains itself.
            let artifact = json!({
                "expression": expression,
                "timeout_secs": timeout.as_secs(),
                "page": diagnostics,
            });
            let _ = std::fs::create_dir_all(directory).and_then(|()| {
                std::fs::write(
                    directory.join(format!("{name}.json")),
                    serde_json::to_vec_pretty(&artifact).unwrap_or_default(),
                )
            });
            bail!(
                "browser timeout after {}s at {}: {expression}\nartifacts: target/browser-artifacts/{name}.{{png,json}}\ndiagnostics: {diagnostics}",
                timeout.as_secs(),
                client
                    .current_url()
                    .await
                    .map(|url| url.to_string())
                    .unwrap_or_else(|error| format!("<unavailable: {error}>"))
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The client marks the document ready once its enhancements are installed; most page contracts
/// start there. The page itself is usable well before this.
pub(crate) async fn wait_booted(client: &Client) -> Result<()> {
    wait_for(
        client,
        "document.documentElement.dataset.aggrReady === 'true'",
    )
    .await
}

pub(crate) async fn wait_booted_with(client: &Client, extra: &str) -> Result<()> {
    wait_for(
        client,
        &format!("document.documentElement.dataset.aggrReady === 'true' && ({extra})"),
    )
    .await
}

/// Releases the session: the contract's own error always wins, and a failure to close the
/// browser is only reported when the contract itself passed.
pub(crate) async fn finish<T>(client: Client, result: Result<T>) -> Result<T> {
    let closed = client.close().await;
    let value = result?;
    closed.context("closing the browser session")?;
    Ok(value)
}

/// Turns an assertion panic inside a contract into an error so the session is still released.
pub(crate) async fn catch_panics<F>(contract: F) -> Result<()>
where
    F: Future<Output = Result<()>>,
{
    std::panic::AssertUnwindSafe(contract)
        .catch_unwind()
        .await
        .unwrap_or_else(|panic| {
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("browser assertion failed");
            Err(anyhow::anyhow!("{message}"))
        })
}

/// Saves a `<name>-failure.png` screenshot and prints the page state when a contract failed.
pub(crate) async fn report_failure(client: &Client, name: &str, result: &Result<()>) {
    if let Err(error) = result {
        let _ = screenshot(client, &format!("{name}-failure")).await;
        eprintln!(
            "{error:#}; screenshot: target/browser-artifacts/{name}-failure.png\nstate: {}",
            page_diagnostics(client).await
        );
    }
}

/// Escape first closes open suggestions; a second press leaves the search field.
pub(crate) async fn escape_search(client: &Client) -> Result<()> {
    key(client, "Escape").await?;
    if client
        .execute("return document.activeElement?.id==='q'", vec![])
        .await?
        == true
    {
        key(client, "Escape").await?;
    }
    Ok(())
}

pub(crate) async fn key(client: &Client, key: &str) -> Result<()> {
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
    // The element holding the keyboard can be replaced as a list re-renders underneath it, so a
    // refused keystroke is worth finding the focus again for.
    for attempt in 0..2 {
        match client.active_element().await?.send_keys(key).await {
            Ok(()) => return Ok(()),
            Err(error) if attempt == 0 => {
                tokio::time::sleep(Duration::from_millis(150)).await;
                let _ = error;
            }
            Err(error) => return Err(error).with_context(|| format!("sending key {key:?}")),
        }
    }
    Ok(())
}

pub(crate) async fn emulate(client: &Client, command: &str, parameters: Value) -> Result<()> {
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

pub(crate) async fn screenshot(client: &Client, name: &str) -> Result<()> {
    let directory = Path::new("target/browser-artifacts");
    std::fs::create_dir_all(directory)?;
    std::fs::write(
        directory.join(format!("{name}.png")),
        client.screenshot().await?,
    )?;
    Ok(())
}

pub(crate) async fn set_offline(client: &Client, offline: bool) -> Result<()> {
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

pub(crate) async fn browser_client() -> Result<Client> {
    browser_client_with_load_strategy("normal").await
}

pub(crate) async fn browser_client_with_load_strategy(strategy: &str) -> Result<Client> {
    browser_client_with_preferences(strategy, json!({})).await
}

pub(crate) async fn browser_client_with_preferences(
    strategy: &str,
    preferences: Value,
) -> Result<Client> {
    let driver = std::env::var("AGGR_WEBDRIVER_URL")
        .context("set AGGR_WEBDRIVER_URL to the local driver")?;
    let mut capabilities = serde_json::Map::new();
    capabilities.insert("browserName".into(), json!("chrome"));
    capabilities.insert("pageLoadStrategy".into(), json!(strategy));
    let mut chrome = json!({"args":["--headless=new", "--no-sandbox", "--disable-dev-shm-usage", "--disable-search-engine-choice-screen"]});
    chrome["prefs"] = preferences;
    if let Ok(binary) = std::env::var("AGGR_CHROME_BINARY") {
        chrome["binary"] = json!(binary);
    }
    capabilities.insert("goog:chromeOptions".into(), chrome);
    let client = ClientBuilder::new(hyper_util::client::legacy::connect::HttpConnector::new())
        .capabilities(capabilities)
        .connect(&driver)
        .await?;
    // Best effort: uncaught client errors are the first thing to read when a wait times out.
    let _ = emulate(
        &client,
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":"window.__aggrErrors=[];addEventListener('error',e=>__aggrErrors.push(String(e.error?.stack||e.message)));addEventListener('unhandledrejection',e=>__aggrErrors.push(String(e.reason)))"}),
    )
    .await;
    Ok(client)
}

/// The 390x844 touch phone every carved reader contract starts from.
pub(crate) async fn phone_session(client: &Client) -> Result<()> {
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
    Ok(())
}
