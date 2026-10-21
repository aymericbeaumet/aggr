use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aggr_ui_build::{BundleOptions, bundle};
use anyhow::{Context, Result, bail, ensure};
use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use fantoccini::{Client, ClientBuilder};
use serde_json::{Value, json};

struct Assets {
    bundles: Vec<String>,
    fixtures: String,
    renderer: String,
}

fn collect(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            collect(&entry.path(), files)?;
        } else if entry.file_name().to_string_lossy().ends_with(".test.ts") {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn compile(root: &Path, files: &[PathBuf]) -> Result<Assets> {
    let helper = root.join("web/test/browser.ts");
    let aliases = BTreeMap::new();
    let mut bundles = Vec::new();
    for file in files {
        let entry = root.join("web/.generated/native-tests/entry.ts");
        let source = format!(
            "import {}; import {{run}} from {}; const selected=new URL(location.href).searchParams.get('case'); await run(selected===null?null:Number(selected));",
            serde_json::to_string(&file.to_string_lossy())?,
            serde_json::to_string(&helper.to_string_lossy())?
        );
        let mut virtual_modules = BTreeMap::from([(entry.clone(), source)]);
        if file.ends_with("search/search.dom.test.ts") {
            virtual_modules.insert(
                root.join("web/src/search/engine.ts"),
                "export * from '../../test/search-engine';".into(),
            );
        }
        if file.ends_with("app/boot-ready.dom.test.ts") {
            virtual_modules.insert(
                root.join("web/src/app/reader.ts"),
                "export * from '../../test/boot-reader';".into(),
            );
            virtual_modules.insert(
                root.join("web/src/navigation/index.ts"),
                "export * from '../../test/boot-navigation';".into(),
            );
        } else {
            // These tests assert the progressive fallback decision without leaving their test page.
            // Record fallback state without unloading the document containing its assertions.
            let navigation = root.join("web/src/navigation/index.ts");
            let original = std::fs::read_to_string(&navigation)?;
            let facade = "const location = {get href(){return globalThis.location.href},get hash(){return globalThis.location.hash},set hash(value){globalThis.location.hash=value},assign(){},replace(){},reload(){}};\n";
            virtual_modules.insert(navigation, format!("{facade}{original}"));
        }
        let output = bundle(
            root,
            BundleOptions {
                entry,
                aliases: aliases.clone(),
                virtual_modules,
                ..Default::default()
            },
        )
        .with_context(|| format!("bundling {}", file.display()))?;
        bundles.push(output.code);
    }
    let entry = root.join("web/.generated/native-tests/renderer.ts");
    let renderer = bundle(
        root,
        BundleOptions {
            entry: entry.clone(),
            server: true,
            global_name: Some("AggrTestRenderer".into()),
            virtual_modules: BTreeMap::from([(
                entry,
            "import {render} from '../../src/server';globalThis.aggrRender=json=>render(JSON.parse(json));".into(),
            )]),
            ..Default::default()
        },
    )?
    .code;
    let mut fixtures = serde_json::Map::new();
    for name in [
        "page-river",
        "page-article",
        "page-browse",
        "page-source-error",
    ] {
        fixtures.insert(
            name.into(),
            serde_json::from_slice::<Value>(&std::fs::read(
                root.join(format!("web/test/fixtures/{name}.json")),
            )?)?,
        );
    }
    Ok(Assets {
        bundles,
        fixtures: serde_json::to_string(&fixtures)?.replace('<', "\\u003c"),
        renderer,
    })
}

async fn serve(State(assets): State<Arc<Assets>>, uri: Uri) -> Response {
    if uri.path() == "/__tests/renderer.js" {
        return (
            [
                (header::CONTENT_TYPE, "text/javascript"),
                (header::CACHE_CONTROL, "public,max-age=31536000,immutable"),
            ],
            assets.renderer.clone(),
        )
            .into_response();
    }
    if let Some(index) = uri
        .path()
        .strip_prefix("/__tests/bundle/")
        .and_then(|p| p.strip_suffix(".js"))
        .and_then(|p| p.parse::<usize>().ok())
    {
        return assets.bundles.get(index).map_or_else(
            || StatusCode::NOT_FOUND.into_response(),
            |code| {
                (
                    [
                        (header::CONTENT_TYPE, "text/javascript"),
                        (header::CACHE_CONTROL, "public,max-age=31536000,immutable"),
                    ],
                    code.clone(),
                )
                    .into_response()
            },
        );
    }
    let file = uri
        .query()
        .and_then(|q| q.split('&').find_map(|part| part.strip_prefix("file=")))
        .and_then(|s| s.parse::<usize>().ok());
    let Some(file) = file.filter(|index| *index < assets.bundles.len()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let html = format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><script>sessionStorage.clear();localStorage.clear();window.__aggrTestFatal=[];addEventListener('error',e=>__aggrTestFatal.push(String(e.error?.stack||e.message)));addEventListener('unhandledrejection',e=>__aggrTestFatal.push(String(e.reason?.stack||e.reason)));globalThis.__aggrFixtures={};</script><script src="/__tests/renderer.js"></script></head><body><script type="module" src="/__tests/bundle/{}.js"></script></body></html>"#,
        assets.fixtures, file
    );
    ([(header::CONTENT_TYPE, "text/html")], html).into_response()
}

fn tool(name: &str, override_name: &str, relative: &[&str]) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(override_name) {
        return Ok(PathBuf::from(path));
    }
    let output = std::process::Command::new("mise")
        .args(["where", name])
        .output()
        .context("locating pinned browser tool")?;
    ensure!(
        output.status.success(),
        "install pinned {name} with mise run setup"
    );
    let root = PathBuf::from(String::from_utf8(output.stdout)?.trim());
    for candidate in relative {
        let path = root.join(candidate);
        if path.is_file() {
            return Ok(path);
        }
    }
    bail!("pinned {name} binary missing below {}", root.display())
}

async fn browser() -> Result<(Client, Option<tokio::process::Child>)> {
    let mut child = None;
    let endpoint = if let Ok(endpoint) = std::env::var("AGGR_WEBDRIVER_URL") {
        endpoint
    } else {
        let binary = tool(
            "http:chromedriver",
            "AGGR_CHROMEDRIVER_BIN",
            &[
                "chromedriver-mac-arm64/chromedriver",
                "chromedriver-mac-x64/chromedriver",
                "chromedriver-linux64/chromedriver",
                "chromedriver-win64/chromedriver.exe",
                "chromedriver",
            ],
        )?;
        let reservation = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = reservation.local_addr()?.port();
        drop(reservation);
        child = Some(
            tokio::process::Command::new(binary)
                .arg(format!("--port={port}"))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()?,
        );
        format!("http://127.0.0.1:{port}")
    };
    let chrome = tool(
        "http:chrome-for-testing",
        "AGGR_CHROME_BINARY",
        &[
            "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
            "chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
            "Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
            "chrome-linux64/chrome",
            "chrome-win64/chrome.exe",
        ],
    )?;
    let capabilities = json!({"browserName":"chrome", "goog:chromeOptions":{"binary":chrome,"args":["--headless=new","--no-sandbox","--disable-dev-shm-usage","--disable-search-engine-choice-screen","--disable-background-networking","--disable-component-update","--no-first-run","--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1, EXCLUDE localhost"]}}).as_object().context("browser capabilities")?.clone();
    let start = Instant::now();
    loop {
        let result = ClientBuilder::new(hyper_util::client::legacy::connect::HttpConnector::new())
            .capabilities(capabilities.clone())
            .connect(&endpoint)
            .await;
        match result {
            Ok(client) => return Ok((client, child)),
            Err(error) if start.elapsed() > Duration::from_secs(10) => return Err(error.into()),
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

async fn result(client: &Client, complete: bool) -> Result<Value> {
    let start = Instant::now();
    loop {
        let state = client
            .execute(
                "return {state:window.__aggrTestResult??null,errors:window.__aggrTestFatal??[]}",
                vec![],
            )
            .await?;
        ensure!(
            state["errors"].as_array().is_none_or(Vec::is_empty),
            "browser test load: {}",
            state["errors"]
        );
        if !state["state"].is_null() && (!complete || !state["state"]["result"].is_null()) {
            return Ok(state["state"].clone());
        }
        ensure!(
            start.elapsed() < Duration::from_secs(10),
            "browser test timed out: {state}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

pub async fn run(args: Vec<String>) -> Result<()> {
    let root = crate::root().canonicalize()?;
    aggr_ui_model::export_types(root.join("web/.generated/types"))?;
    let mut files = Vec::new();
    collect(&root.join("web/src"), &mut files)?;
    files.push(root.join("web/test/browser.test.ts"));
    files.sort();
    if !args.is_empty() {
        files.retain(|path| args.iter().any(|arg| path.to_string_lossy().contains(arg)));
    }
    ensure!(!files.is_empty(), "no frontend tests matched");
    let build_root = root.clone();
    let build_files = files.clone();
    let assets =
        Arc::new(tokio::task::spawn_blocking(move || compile(&build_root, &build_files)).await??);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(
        axum::serve(listener, Router::new().fallback(serve).with_state(assets)).into_future(),
    );
    let (client, mut driver) = browser().await?;
    let outcome: Result<()> = async {
        client.set_window_rect(0, 0, 1280, 1000).await?;
        let mut count = 0;
        let mut failures = Vec::new();
        for (file, path) in files.iter().enumerate() {
            client.goto(&format!("{origin}/?file={file}")).await?;
            let listed = result(&client, false).await?;
            let cases = listed["cases"]
                .as_array()
                .context("missing browser cases")?;
            ensure!(!cases.is_empty(), "no cases in {}", path.display());
            for index in 0..cases.len() {
                client
                    .goto(&format!("{origin}/?file={file}&case={index}"))
                    .await?;
                let tested = result(&client, true).await?;
                if let Some(error) = tested["result"]["error"].as_str() {
                    failures.push(format!(
                        "{}: {}\n{error}",
                        path.strip_prefix(&root)?.display(),
                        tested["result"]["name"]
                    ));
                }
                count += 1;
            }
            println!(
                "{}: {} cases",
                path.strip_prefix(&root)?.display(),
                cases.len()
            );
        }
        for failure in &failures {
            eprintln!("FAIL {failure}");
        }
        ensure!(
            failures.is_empty(),
            "{} of {count} frontend cases failed",
            failures.len()
        );
        println!(
            "{count} frontend cases passed across {} files in real Chrome",
            files.len()
        );
        Ok(())
    }
    .await;
    let _ = client.close().await;
    if let Some(driver) = driver.as_mut() {
        let _ = driver.kill().await;
    }
    server.abort();
    outcome
}
