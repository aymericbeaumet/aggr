//! Browser contracts against a generated, pinned archive. Run with a local WebDriver:
//! AGGR_WEBDRIVER_URL=http://127.0.0.1:9515 cargo test --test browser -- --ignored

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
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
    stopped: Arc<AtomicBool>,
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
    fn deploy_update(&self) -> Result<()> {
        let root = self._directory.path();
        let config = root.join("aggr.toml");
        std::fs::write(
            &config,
            std::fs::read_to_string(&config)?.replace("Reading room", "Updated reading room"),
        )?;
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
        Ok(())
    }

    fn new() -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let base = format!("http://{}/reader/", listener.local_addr()?);
        git(root, &["init", "-q", "-b", "main"])?;
        std::fs::write(
            root.join("aggr.toml"),
            "[site]\ntitle='Reading room'\nitems_per_page=3\nmax_age_days=10000\noffline_items=4\n[[sources]]\nurl='https://publisher.invalid/feed'\nslug='example'\nname='Example'\ncategory='Engineering'\n[[networks]]\nprovider='hackernews'\n[[networks]]\nprovider='reddit'\n",
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
                _ => format!("https://publisher.invalid/story-{index}"),
            };
            let updated = if index == 44 {
                &published
            } else {
                "2026-09-05T12:00:00Z"
            };
            let markdown = format!(
                "---\ntitle: {title}\nlink: {link}\nsource: example\npublished: {published}\nupdated: {updated}\nfirst_seen: {published}\ncontent: feed\nlabels: [reading, rust]\n{preview}{archived_image}---\n\n* * *\n\nA paragraph with [first link](https://example.invalid/one) and more prose before [a comparison grid](https://example.invalid/two) continues naturally.\n\n```bash\n$ z dotfiles\n$ pwd\n/private/dotfiles\n```\n\n![An article illustration]({base}body.png)\n\n{}\n",
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
        let offline = Arc::new(AtomicBool::new(false));
        let scripts_blocked = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let serving = out.clone();
        let is_offline = offline.clone();
        let block_scripts = scripts_blocked.clone();
        let is_stopped = stopped.clone();
        listener.set_nonblocking(true)?;
        std::thread::spawn(move || {
            while !is_stopped.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let root = serving.clone();
                        let offline = is_offline.load(Ordering::Relaxed);
                        let scripts_blocked = block_scripts.load(Ordering::Relaxed);
                        std::thread::spawn(move || {
                            let _ = serve(stream, &root, offline, scripts_blocked);
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
            stopped,
        })
    }
}

fn serve(mut stream: TcpStream, root: &Path, offline: bool, scripts_blocked: bool) -> Result<()> {
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
    let accepts_html = request.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("accept") && value.contains("text/html")
        })
    });
    let relative = path
        .strip_prefix("/reader/")
        .or_else(|| path.strip_prefix("/other/"));
    let (status, mime, body) = if offline {
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
        "ArrowRight" => "\u{e014}",
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
    let driver = std::env::var("AGGR_WEBDRIVER_URL")
        .context("set AGGR_WEBDRIVER_URL to the local driver")?;
    let mut capabilities = serde_json::Map::new();
    capabilities.insert("browserName".into(), json!("chrome"));
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
        eprintln!("state: {}", client.execute("return {url:location.href,online:navigator.onLine,swup:!!window.swup,controller:navigator.serviceWorker.controller?.scriptURL,status:document.querySelector('#connection-status')?.textContent,scripts:Array.from(document.scripts).map(s=>s.src)}", vec![]).await.unwrap_or(Value::Null));
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

async fn run_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
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
      const target = new URL('library/?prefetch-contract=1',document.baseURI);
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
        destination:new URL('library/?prefetch-cancellation=destination',document.baseURI).href,
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
    wait_for(client, "document.body.dataset.kind === 'library' && window.prefetchCancellationContract.settled.destination === 'fulfilled'").await?;
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
          const player=document.querySelector('.video-player'), replace=player.replaceChildren;
          player.replaceChildren=function(frame){
            window.videoContract={url:frame.src,allow:frame.allow,sandbox:frame.getAttribute('sandbox'),referrer:frame.referrerPolicy,title:frame.title};
            frame.removeAttribute('src');
            replace.call(this,frame);
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
      const nav = document.querySelector('.mobile-nav');
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
      const rankText = document.createRange();
      rankText.selectNodeContents(rank);
      const rankTextRect = rankText.getBoundingClientRect();
      const rankRights = Array.from(document.querySelectorAll('.rank')).map(node => node.getBoundingClientRect().right);
      const rankTitle = rank.parentElement.querySelector('.title');
      const originalRank = rank.textContent;
      rank.textContent = '10.';
      const twoDigitRankText = document.createRange();
      twoDigitRankText.selectNodeContents(rank);
      const twoDigitRankTextRect = twoDigitRankText.getBoundingClientRect();
      const rankGap = rankTitle.getBoundingClientRect().left - twoDigitRankTextRect.right;
      rank.textContent = originalRank;
      return {count:nav.querySelectorAll('a').length, visible:getComputedStyle(nav).display !== 'none',
        overflow:document.documentElement.scrollWidth > document.documentElement.clientWidth + 1,
        reload:!!document.querySelector('#refresh-page'), density:document.documentElement.dataset.density,
        configVisible:getComputedStyle(config).display !== 'none',
        configRight:innerWidth-config.getBoundingClientRect().right,
        selected:document.querySelectorAll('.row.is-selected').length,
        brandLeft:brand.getBoundingClientRect().left, rankLeft:rank.getBoundingClientRect().left,
        rankWidth:rank.getBoundingClientRect().width, rankTextLeft:rankTextRect.left,
        rankTextRight:rankTextRect.right, rankTextWidth:rankTextRect.width,
        twoDigitRankTextRight:twoDigitRankTextRect.right, rankGap,
        titleLeft:title.getBoundingClientRect().left,metaLeft:meta.getBoundingClientRect().left,
        rankRightSpread:Math.max(...rankRights)-Math.min(...rankRights), rankFont:getComputedStyle(rank).fontFamily,
        header:{height:top.getBoundingClientRect().height,position:topStyle.position,
          background:topStyle.backgroundColor,navMinHeight:topNavStyle.minHeight,
          navPaddingLeft:topNavStyle.paddingLeft,navPaddingRight:topNavStyle.paddingRight,
          brandMarginLeft:brandLinkStyle.marginLeft,brandMarginRight:brandLinkStyle.marginRight,
          configFontSize:configStyle.fontSize,configPaddingLeft:configStyle.paddingLeft,
          configPaddingRight:configStyle.paddingRight},
        rowHeight:row.getBoundingClientRect().height,
        rowInset:title.getBoundingClientRect().left-row.getBoundingClientRect().left,
        rowPadding:parseFloat(getComputedStyle(row).paddingTop)};
    "#, vec![]).await?;
    assert_eq!(layout["count"], 4);
    assert_eq!(layout["visible"], true);
    assert_eq!(layout["overflow"], false);
    assert_eq!(layout["reload"], false);
    assert_eq!(layout["density"], "compact");
    assert_eq!(layout["configVisible"], true);
    assert!(
        layout["configRight"].as_f64().unwrap_or_default() <= 17.0,
        "aggr.toml should stay at the mobile header's top-right edge: {layout}"
    );
    assert_eq!(layout["selected"], 1, "the first feed row starts selected");
    assert!(
        (layout["rankTextLeft"].as_f64().unwrap_or_default()
            - layout["brandLeft"].as_f64().unwrap_or_default()
            - 8.0)
            .abs()
            <= 1.0,
        "rows should reserve the same extra 8px inset on mobile and desktop: {layout}"
    );
    assert!(
        layout["rankRightSpread"].as_f64().unwrap_or(f64::MAX) <= 0.5
            && layout["rankFont"]
                .as_str()
                .unwrap_or_default()
                .contains("ui-monospace"),
        "entry ranks need a fixed, right-aligned monospace units column: {layout}"
    );
    assert!(
        layout["titleLeft"].as_f64().unwrap_or_default()
            > layout["rankTextRight"].as_f64().unwrap_or_default(),
        "the mobile title should follow the rank on the same row: {layout}"
    );
    assert!(
        (layout["metaLeft"].as_f64().unwrap_or_default()
            - layout["titleLeft"].as_f64().unwrap_or_default())
        .abs()
            <= 1.0,
        "mobile metadata should share the title text axis: {layout}"
    );
    assert!(
        (layout["rankTextRight"].as_f64().unwrap_or_default()
            - layout["twoDigitRankTextRight"].as_f64().unwrap_or_default())
        .abs()
            <= 0.5,
        "one- and two-digit ranks must share their right edge: {layout}"
    );
    assert!(
        layout["rankGap"].as_f64().unwrap_or_default() >= 6.0,
        "two-digit ranks need visible space before the title: {layout}"
    );
    let compact_row_height = layout["rowHeight"].as_f64().context("compact row height")?;
    assert!(
        compact_row_height <= 126.0,
        "compact mobile row should stay dense: {layout}"
    );
    assert!(
        (layout["rowInset"].as_f64().unwrap_or_default() - 48.0).abs() <= 1.0,
        "feed titles should follow the rank inside their selection surface: {layout}"
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
    wait_for(client, "window.swup.cache.has('/reader/search/') && window.swup.cache.has(document.querySelector('[data-row-open]').href)").await?;
    screenshot(client, "mobile-feed").await?;

    for (route, path, title) in [
        (
            "library/",
            "/reader/library/",
            "library | Reading room | aggr",
        ),
        ("search/", "/reader/search/", "search | Reading room | aggr"),
        (
            "preferences/",
            "/reader/preferences/",
            "preferences | Reading room | aggr",
        ),
        ("", "/reader/", "Reading room | aggr"),
    ] {
        let selector = format!(".mobile-nav a[data-route='{route}']");
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
        if route == "search/" {
            wait_for(client, "document.activeElement?.id === 'q'").await?;
            client.find(Locator::Css(&selector)).await?.click().await?;
            wait_for(client, "document.activeElement?.id === 'q'").await?;
        }
    }

    key(client, "\u{e03d}k\u{e000}").await?;
    wait_for(
        client,
        "location.pathname === '/reader/search/' && document.activeElement?.id === 'q'",
    )
    .await?;
    for directory in [
        "library",
        "sources/example",
        "tags/reading",
        "categories/engineering",
    ] {
        let destination = format!("{}{directory}/", fixture.base);
        client.goto(&destination).await?;
        wait_for(
            client,
            "document.documentElement.classList.contains('swup-enabled')",
        )
        .await?;
        assert_eq!(
            client.execute("return document.querySelector('[data-page-search], .directory-count') === null", vec![]).await?,
            true,
            "Library and collection pages must not have local search controls"
        );
        key(client, "/").await?;
        wait_for(
            client,
            "location.pathname === '/reader/search/' && document.activeElement?.id === 'q'",
        )
        .await?;
    }
    for route in ["", "library/", "search/", "preferences/"] {
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
            "!!window.swup && !!document.querySelector('.row [data-row-open]')",
        )
        .await?;
        assert_eq!(client.execute("return document.querySelector('.row.is-selected') === document.querySelector('.row:not([hidden])') && !document.activeElement.matches('[data-row-open]') && scrollY === 0", vec![]).await?, true,
            "initial selection marks the first visible row without moving focus or scrolling");
        if !mobile {
            for (selector, decoration) in [(".row .rank", "underline"), (".row .domain", "none")] {
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
          rank.dispatchEvent(new MouseEvent('click', {bubbles:true,cancelable:true,ctrlKey:true,shiftKey:true}));
          rank.dispatchEvent(new MouseEvent('auxclick', {bubbles:true,cancelable:true,button:1}));
          const range = document.createRange();
          range.selectNodeContents(title);
          getSelection().removeAllRanges();
          getSelection().addRange(range);
          rank.dispatchEvent(new MouseEvent('click', {bubbles:true,cancelable:true}));
          getSelection().removeAllRanges();
          title.removeEventListener('click', capture);
          title.removeEventListener('auxclick', capture);
          const after = {row:row.getBoundingClientRect().left, title:title.getBoundingClientRect().left, rank:rank.getBoundingClientRect().left, background:getComputedStyle(row).backgroundColor};
          const marker = getComputedStyle(row, '::before');
          const original = row.querySelector('.u-bookmark-of'), field = original.parentElement;
          const linkRect = original.getBoundingClientRect(), fieldRect = field.getBoundingClientRect();
          const separator = getComputedStyle(field, '::before');
          const hit = document.elementFromPoint((fieldRect.left + linkRect.left) / 2, linkRect.top + linkRect.height / 2);
          return {forwarded, before, after, marker:{width:marker.width,opacity:marker.opacity,transition:marker.transitionDuration,display:marker.display}, separator:{outsideLink:!hit.closest('a'),balanced:separator.marginLeft===separator.marginRight}, article:title.href, source:row.querySelector('.domain').href};
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
            json!({"outsideLink":true,"balanced":true}),
            "middots and equal surrounding space must remain outside metadata links"
        );
        client
            .find(Locator::Css(".row .domain"))
            .await?
            .click()
            .await?;
        wait_for(
            client,
            &format!("location.href === {}", row_actions["source"]),
        )
        .await?;
        client.goto(&fixture.base).await?;
        wait_for(
            client,
            "!!window.swup && !!document.querySelector('.row .rank')",
        )
        .await?;
        client
            .find(Locator::Css(".row .rank"))
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
    wait_for(client, "location.pathname === '/reader/library/'").await?;
    client.goto(&fixture.base).await?;
    wait_for(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3 && document.documentElement.classList.contains('swup-enabled')",
    )
    .await?;
    client.execute("sessionStorage.setItem('aggr:last-seen-entry:' + encodeURIComponent('/reader/'), document.querySelectorAll('[data-row-open]')[1].href)", vec![]).await?;
    client.refresh().await?;
    wait_for(client, "!!document.querySelector('.row.is-new') && document.documentElement.classList.contains('swup-enabled')").await?;
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
      const row = document.querySelector('.row.is-new');
      const cell = row.querySelector('.cell');
      const background = getComputedStyle(cell, '::before');
      const separator = getComputedStyle(row, '::after');
      const aligned = background.left === separator.left && background.right === separator.right;
      const color = () => getComputedStyle(cell, '::before').backgroundColor;
      const highlighted = color();
      row.classList.remove('is-new');
      requestAnimationFrame(() => {
        const initial = color();
        setTimeout(() => {
          const middle = color();
          setTimeout(() => {
            const final = color();
            row.classList.add('is-new');
            done({highlighted, initial, middle, final, aligned});
          }, 800);
        }, 250);
      });
    "#,
            vec![],
        )
        .await?;
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
        .execute("document.querySelector('.row .domain').focus()", vec![])
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
      return {code:body.querySelector('pre code').textContent, link:getComputedStyle(body.querySelector('p a + a')).display,
        headerMeta:head.querySelector('.meta').textContent.replace(/\s+/g, ' ').trim(),
        imageLoading:image.getAttribute('loading'), imageSource:image.getAttribute('src'), imageClass:image.className,
        imagePriority:image.getAttribute('fetchpriority'), imagePlaceholder:picture.style.getPropertyValue('--image-placeholder'),
        imagePreview:picture.dataset.placeholder, pictureLoaded:picture.classList.contains('is-loaded'),
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
        separateTags:!head.querySelector('.meta .item-tags'), metadataOrder:Array.from(head.querySelector('.meta').children).map(node => node.firstElementChild.className), categoryBelow:!!head.querySelector('.item-tags a[href*="categories/"]'), categoryText:head.querySelector('.meta .category')?.textContent, categoryUrl:head.querySelector('.meta .category a')?.pathname, leadingRule:getComputedStyle(body.querySelector('hr:first-child')).display,
        outline:getComputedStyle(document.querySelector('main')).outlineStyle,
        highlighted:!!body.querySelector('pre span'), bodyLeft:body.getBoundingClientRect().left, bodyWidth:body.getBoundingClientRect().width,
        footerLeft:footer.left, footerWidth:footer.width, moreLeft:more.left, moreWidth:more.width,
        moreCount:moreCards.length, moreClasses:Array.from(new Set(moreCards.map(card => card.className))).length,
        moreStacked:moreBoxes.length < 2 || moreBoxes[1].top > moreBoxes[0].bottom};
    "#, vec![]).await?;
    assert_eq!(article["code"], "$ z dotfiles\n$ pwd\n/private/dotfiles\n");
    assert_eq!(article["link"], "inline");
    assert!(
        article["headerMeta"]
            .as_str()
            .unwrap_or_default()
            .contains("published ")
            && !article["headerMeta"]
                .as_str()
                .unwrap_or_default()
                .contains("Published "),
        "article publication label should be lowercase: {article}"
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
    assert_eq!(article["categoryUrl"], "/reader/categories/engineering/");
    assert_eq!(article["imageLoading"], "eager");
    assert_eq!(article["imagePriority"], "high");
    assert_eq!(article["imageClass"], "progressive-image");
    assert_eq!(article["imagePlaceholder"], "#315d76");
    assert!(
        article["imagePreview"]
            .as_str()
            .unwrap_or_default()
            .starts_with("assets/images/"),
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
        article["indent"].as_f64().unwrap_or_default() > 0.0,
        "paragraphs need a first-line indent: {article}"
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
            .is_some_and(|duration| duration.starts_with("PT") && duration.ends_with('M')),
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
        const line = getComputedStyle(head, '::after');
        return {progress:parseFloat(head.style.getPropertyValue('--reading-progress')),
          fill:new DOMMatrixReadOnly(line.transform).a, transition:line.transitionDuration,
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
      const bottom = document.querySelector('.mobile-nav').getBoundingClientRect().height;
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
        "document.querySelectorAll('[data-preference]').length === 9",
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
            "text-size",
            "reading-width",
            "density",
            "thumbnails",
            "feed-page-size",
            "date-format",
            "single-key-shortcuts"
        ])
    );
    assert_eq!(
        preference_controls["groups"],
        json!(["Appearance", "Reading", "Feed", "Keyboard"])
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
      row.classList.add('is-selected');
      return {
        height:row.getBoundingClientRect().height,
        inset:title.getBoundingClientRect().left-row.getBoundingClientRect().left,
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
            && (comfortable_row["inset"].as_f64().unwrap_or_default() - 48.0).abs() <= 1.0,
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
        "document.querySelectorAll('[data-preference]').length === 9",
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
        .execute(
            "document.querySelector('.preferences-transfer').open=true;document.querySelector('#copy-state').click();",
            vec![],
        )
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
        "!document.querySelector('#preferences-import').hidden && document.querySelectorAll('#preferences-import-summary li').length === 9",
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
    assert_eq!(checks["checks"], 17);
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
            "localStorage.setItem('aggr:single-key-shortcuts','true')",
            vec![],
        )
        .await?;
    client
        .goto(&format!("{}search/?q=article", fixture.base))
        .await?;
    wait_for(
        client,
        "document.querySelectorAll('#list .row').length === 40",
    )
    .await?;
    let input = client.find(Locator::Css("#q")).await?;
    input.clear().await?;
    input.send_keys("j").await?;
    assert_eq!(input.prop("value").await?, Some("j".into()));
    input.clear().await?;
    input.send_keys("article").await?;
    wait_for(
        client,
        "document.querySelector('#q').value === 'article' && document.querySelector('#list').getAttribute('aria-busy') === 'false' && document.querySelectorAll('#list .row').length === 40",
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
    wait_for(client, "document.querySelectorAll('#list .row').length === 40 && !!document.activeElement.matches('[data-row-open]')").await?;
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
            mobile:getComputedStyle(document.querySelector('.mobile-nav')).display !== 'none'};
        "#,
                vec![],
            )
            .await?;
        assert_eq!(
            geometry,
            json!({"overflow":false,"mobile":width<=640}),
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
      const domain = row.querySelector('.domain').getBoundingClientRect();
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
        sourceBelowTitle:domain.top >= title.bottom - 1,
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
    assert_eq!(desktop_density["sourceBelowTitle"], true);
    assert_eq!(
        desktop_density["header"], layout["header"],
        "mobile and desktop top bars should share all visual geometry and color: mobile={}, desktop={}",
        layout["header"], desktop_density["header"]
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
        client.execute("return {overflow:[...document.querySelectorAll('body *')].map(e=>({tag:e.tagName,cls:e.className,right:e.getBoundingClientRect().right,width:e.getBoundingClientRect().width})).filter(e=>e.right>document.documentElement.clientWidth+1).slice(0,12),layout:['html','body','.main','.rows','.row','.mobile-nav'].map(s=>{const e=document.querySelector(s),c=getComputedStyle(e),r=e.getBoundingClientRect();return {s,left:r.left,width:r.width,pad:c.padding,margin:c.margin,font:c.fontSize}})}",vec![]).await?
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
    fixture.deploy_update()?;
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
    client
        .find(Locator::Css(".mobile-nav a[data-route='']"))
        .await?
        .click()
        .await?;
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
        .execute("window.swup.destroy(); delete window.swup", vec![])
        .await?;
    client
        .find(Locator::Css(".mobile-nav a[data-route='preferences/']"))
        .await?
        .click()
        .await?;
    wait_for(
        client,
        "location.pathname.endsWith('/preferences/') && !!document.querySelector('#theme-mode')",
    )
    .await?;
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
    client
        .find(Locator::Css(".mobile-nav a[data-route='search/']"))
        .await?
        .click()
        .await?;
    wait_for(client, "location.pathname === '/reader/search/'").await?;
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
