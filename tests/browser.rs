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
            let markdown = format!(
                "---\ntitle: {title}\nlink: https://publisher.invalid/story-{index}\nsource: example\npublished: {published}\nfirst_seen: {published}\ncontent: feed\nlabels: [reading, rust]\n{preview}{archived_image}---\n\nA paragraph with [first link](https://example.invalid/one) and more prose before [a comparison grid](https://example.invalid/two) continues naturally.\n\n```bash\n$ z dotfiles\n$ pwd\n/private/dotfiles\n```\n\n![An article illustration]({base}body.png)\n\n{}\n",
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
    client.active_element().await?.send_keys(key).await?;
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
        json!({"status":"page 2 / 3","previous":false,"nextPath":"/reader/page/2/","nextSlice":false,"nextHidden":false})
    );
    assert_eq!(
        feed_page_size["finalState"],
        json!({"status":"page 3 / 3","previousPath":"/reader/","previousSlice":"2","nextHidden":true})
    );
    assert_eq!(feed_page_size["defaultVisible"], 25);
    assert_eq!(feed_page_size["defaultPagerHidden"], false);
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
      const domain = rank.parentElement.querySelector('.domain');
      const originalRank = rank.textContent;
      rank.textContent = '10.';
      const twoDigitRankText = document.createRange();
      twoDigitRankText.selectNodeContents(rank);
      const twoDigitRankTextRect = twoDigitRankText.getBoundingClientRect();
      const rankGap = domain.getBoundingClientRect().left - twoDigitRankTextRect.right;
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
    assert_eq!(layout["selected"], 0, "feed rows start unselected");
    assert!(
        (layout["brandLeft"].as_f64().unwrap_or_default()
            - layout["rankTextLeft"].as_f64().unwrap_or_default())
        .abs()
            <= 1.0,
        "the logo and visible one-digit ranks must share a vertical axis: {layout}"
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
        (layout["rankTextLeft"].as_f64().unwrap_or_default()
            - layout["titleLeft"].as_f64().unwrap_or_default())
        .abs()
            <= 1.0,
        "the mobile rank/source line should begin on the title axis: {layout}"
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
        "two-digit ranks need visible space before the source: {layout}"
    );
    let compact_row_height = layout["rowHeight"].as_f64().context("compact row height")?;
    assert!(
        compact_row_height <= 126.0,
        "compact mobile row should stay dense: {layout}"
    );
    assert!(
        (layout["rowInset"].as_f64().unwrap_or_default() - 12.0).abs() <= 1.0,
        "feed rows should retain a comfortable inset inside their selection surface: {layout}"
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
        closeDecoration:getComputedStyle(close).textDecorationLine};
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
    key(client, "Escape").await?;
    wait_for(client, "!document.querySelector('#shortcut-help').open").await?;
    wait_for(
        client,
        "document.querySelector('.preview-image')?.naturalWidth === 240",
    )
    .await?;
    wait_for(
        client,
        "document.documentElement.classList.contains('swup-native')",
    )
    .await?;
    let layers = client.execute(r#"
      return {root:getComputedStyle(document.documentElement).viewTransitionName,
        main:getComputedStyle(document.querySelector('.main')).viewTransitionName,
        nav:getComputedStyle(document.querySelector('.mobile-nav')).viewTransitionName,
        rootLayer:getComputedStyle(document.documentElement,'::view-transition-group(root)').zIndex,
        navLayer:getComputedStyle(document.documentElement,'::view-transition-group(reader-navigation)').zIndex,
        rootOldAnimation:getComputedStyle(document.documentElement,'::view-transition-old(root)').animationName,
        rootNewAnimation:getComputedStyle(document.documentElement,'::view-transition-new(root)').animationName,
        navAnimation:getComputedStyle(document.documentElement,'::view-transition-new(reader-navigation)').animationName};
    "#, vec![]).await?;
    assert_eq!(
        layers,
        json!({"root":"root","main":"none","nav":"reader-navigation","rootLayer":"1","navLayer":"3",
          "rootOldAnimation":"reader-fade-out","rootNewAnimation":"reader-fade-in","navAnimation":"none"})
    );
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
    }

    key(client, "/").await?;
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
        wait_for(client, "!!document.querySelector('[data-page-search]')").await?;
        key(client, "/").await?;
        wait_for(
            client,
            "document.activeElement?.matches('[data-page-search]')",
        )
        .await?;
        assert_eq!(
            client.current_url().await?.path(),
            format!("/reader/{directory}/"),
            "slash must keep Library and collection searches local"
        );
        if directory == "library" {
            client
                .find(Locator::Css("[data-page-search]"))
                .await?
                .send_keys("example")
                .await?;
            wait_for(
                client,
                "new URL(location.href).searchParams.get('q') === 'example'",
            )
            .await?;
            client.refresh().await?;
            wait_for(
                client,
                "document.querySelector('[data-page-search]')?.value === 'example'",
            )
            .await?;
        }
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
        0,
        "an ordinary reload must not restore a visual row selection"
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
    key(client, "j").await?;
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
        "the selected row background is sufficient without underlining its title"
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
      window.__aggrTransitionCapture = {installed:true};
      const start = document.startViewTransition.bind(document);
      document.startViewTransition = function (update) {
        const transition = start(update);
        transition.ready.then(function () {
          const root = getComputedStyle(document.documentElement, '::view-transition-group(root)');
          const oldRoot = getComputedStyle(document.documentElement, '::view-transition-old(root)');
          const newRoot = getComputedStyle(document.documentElement, '::view-transition-new(root)');
          window.__aggrTransitionCapture = {
            ready:true,
            viewportWidth:innerWidth,
            viewportHeight:innerHeight,
            width:parseFloat(root.width),
            height:parseFloat(root.height),
            transform:root.transform,
            groupAnimation:root.animationName,
            oldAnimation:oldRoot.animationName,
            newAnimation:newRoot.animationName,
            main:getComputedStyle(document.querySelector('.main')).viewTransitionName
          };
          transition.finished.then(function () { window.__aggrTransitionCapture.finished = true; });
        });
        return transition;
      };
    "#,
            vec![],
        )
        .await?;
    key(client, "o").await?;
    wait_for(
        client,
        "!!document.querySelector('.body pre') && window.__aggrTransitionCapture?.ready",
    )
    .await?;
    let transition = client
        .execute("return window.__aggrTransitionCapture", vec![])
        .await?;
    assert_eq!(transition["main"], "none");
    assert_eq!(transition["groupAnimation"], "none");
    assert_eq!(transition["oldAnimation"], "reader-fade-out");
    assert_eq!(transition["newAnimation"], "reader-fade-in");
    assert!(
        (transition["width"].as_f64().unwrap_or_default()
            - transition["viewportWidth"].as_f64().unwrap_or_default())
        .abs()
            <= 1.0,
        "root transition snapshot must keep viewport width: {transition}"
    );
    assert!(
        (transition["height"].as_f64().unwrap_or_default()
            - transition["viewportHeight"].as_f64().unwrap_or_default())
        .abs()
            <= 1.0,
        "root transition snapshot must keep viewport height: {transition}"
    );
    let transform = transition["transform"].as_str().unwrap_or_default();
    assert!(
        transform == "none" || transform == "matrix(1, 0, 0, 1, 0, 0)",
        "root transition must not translate the scrolled page: {transition}"
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
    for axis in ["headTop", "titleTop", "titleLeft"] {
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
    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":390,"height":844,"deviceScaleFactor":1,"mobile":true}),
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
    assert_eq!(
        article["mainPaddingLeft"], 20.0,
        "mobile articles should keep a 20px reading gutter: {article}"
    );
    assert!(
        article["readingStats"]
            .as_str()
            .is_some_and(|text| text.contains(" words · ") && text.ends_with(" min read")),
        "article metadata should expose word count and reading time: {article}"
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
    for axis in ["top", "titleTop", "titleLeft"] {
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
            && (comfortable_row["inset"].as_f64().unwrap_or_default() - 12.0).abs() <= 1.0,
        "comfortable selected rows need padding without losing alignment: {comfortable_row}"
    );
    assert_ne!(
        comfortable_row["plain"], comfortable_row["selected"],
        "selection needs a visible but label-free background"
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
    assert_eq!(checks["checks"], 14);
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
            "localStorage.setItem('aggr:density','compact');localStorage.setItem('aggr:text-size','normal')",
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
        sameLine:Math.abs(title.bottom-domain.bottom) <= 4,
        metaLeft:meta.left,titleLeft:title.left,separatorLeft,
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
    assert_eq!(desktop_density["sameLine"], true);
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
            - desktop_density["brandLeft"].as_f64().unwrap_or_default())
        .abs()
            <= 1.0,
        "desktop ranks should begin on the brand/content axis without an empty gutter: {desktop_density}"
    );
    assert!(
        (desktop_density["rankTextLeft"].as_f64().unwrap_or_default()
            - desktop_density["separatorLeft"]
                .as_f64()
                .unwrap_or_default())
        .abs()
            <= 1.0,
        "desktop ranks, separators, and the brand icon should share one left axis: {desktop_density}"
    );
    screenshot(client, "desktop-feed").await?;
    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":320,"height":844,"deviceScaleFactor":1,"mobile":true}),
    )
    .await?;
    client
        .execute("document.documentElement.style.fontSize='200%'", vec![])
        .await?;
    assert_eq!(
        client
            .execute(
                "return document.documentElement.scrollWidth <= document.documentElement.clientWidth+1",
                vec![]
            )
            .await?,
        true
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
        .execute("window.readingSentinel=42;window.scrollTo(0,300)", vec![])
        .await?;
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
