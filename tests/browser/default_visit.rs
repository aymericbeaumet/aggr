//! A default visit keeps optional archive downloads dormant until the reader asks for them.

use anyhow::Result;
use fantoccini::Locator;

use crate::harness::{
    Fixture, browser_client, finish, phone_session, report_failure, screenshot, wait_booted,
    wait_for,
};

fn default_fixture() -> Result<Fixture> {
    let fixture = Fixture::new()?;
    let config = fixture.directory.path().join("aggr.toml");
    let text = std::fs::read_to_string(&config)?;
    std::fs::write(
        &config,
        text.replace("preferences.offline_items=4\n", "")
            .replace("items_per_page=3\n", "")
            .replace(crate::harness::ARCHIVED_MEDIA_SETTINGS, ""),
    )?;
    fixture.build()?;
    Ok(fixture)
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn first_visit_reuses_content_addressed_assets_from_the_http_cache() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = default_fixture()?;
    fixture
        .media
        .cache_assets
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let client = browser_client().await?;
    let result = async {
        client.goto(&fixture.base).await?;
        wait_booted(&client).await?;
        wait_for(&client, "!!navigator.serviceWorker.controller").await?;
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        let duplicate_assets: Vec<_> = fixture.media.asset_requests.lock().expect("fixture asset requests")
            .iter().filter(|(_, count)| **count > 1).map(|(path, count)| (path.clone(), *count)).collect();
        anyhow::ensure!(duplicate_assets.is_empty(), "worker installation must reuse cacheable assets: {duplicate_assets:?}");
        let bytes = fixture.served_bytes.load(std::sync::atomic::Ordering::Relaxed);
        eprintln!("first visit with cacheable assets: {bytes} uncompressed HTTP response bytes including worker installation");
        anyhow::ensure!(bytes <= 768 * 1024, "cacheable fixture visit exceeded 768 KiB: {bytes} bytes");
        Ok(())
    }.await;
    report_failure(&client, "cached-first-visit", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn reader_introduction_collapses_and_survives_navigation() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let config = fixture.directory.path().join("aggr.toml");
    let text = std::fs::read_to_string(&config)?;
    std::fs::write(
        &config,
        format!("{text}\n[site.params]\nintroduction=true\n"),
    )?;
    fixture.build()?;
    let client = browser_client().await?;
    let result = async {
        client.set_window_rect(0, 0, 1280, 900).await?;
        client.goto(&fixture.base).await?;
        wait_booted(&client).await?;
        anyhow::ensure!(client.execute("return document.querySelector('.reader-intro')?.open === true", vec![]).await? == true);
        screenshot(&client, "reader-intro-desktop").await?;
        phone_session(&client).await?;
        screenshot(&client, "reader-intro-mobile").await?;
        anyhow::ensure!(client.execute("return document.documentElement.scrollWidth <= innerWidth", vec![]).await? == true, "reader introduction must fit a phone");
        client.find(Locator::Css(".reader-intro summary")).await?.click().await?;
        client.find(Locator::Css(".rows .row [data-row-open]")).await?.click().await?;
        wait_for(&client, "document.body.dataset.kind === 'item'").await?;
        anyhow::ensure!(client.execute("return getComputedStyle(document.querySelector('.reader-intro')).display === 'none'", vec![]).await? == true);
        client.back().await?;
        wait_for(&client, "document.body.dataset.kind === 'river'").await?;
        anyhow::ensure!(client.execute("return document.querySelector('.reader-intro')?.open === false", vec![]).await? == true, "the introduction stays collapsed after reading an article");
        client.goto(&format!("{}browse/", fixture.base)).await?;
        wait_booted(&client).await?;
        anyhow::ensure!(client.execute("return document.querySelectorAll('.reading-list-actions a').length === 3 && document.documentElement.scrollWidth <= innerWidth", vec![]).await? == true, "subscription actions stay available on a phone");
        screenshot(&client, "reading-list-mobile").await?;
        Ok(())
    }.await;
    report_failure(&client, "reader-introduction", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn default_visit_downloads_only_the_reader_shell_until_intent() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = default_fixture()?;
    let client = browser_client().await?;
    let result = async {
        client.goto(&fixture.base).await?;
        wait_booted(&client).await?;
        wait_for(&client, "!!navigator.serviceWorker.controller").await?;
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        let idle = client.execute_async(r#"
          const done = arguments[arguments.length - 1];
          (async () => {
            const requested = performance.getEntriesByType('resource').map(entry => new URL(entry.name).pathname);
            const visible = new Set([...document.querySelectorAll('.rows .row [data-row-open]')].map(link => new URL(link.href).pathname));
            const lazy = path => /\/(search|media|Form)-[^/]+\.js$/.test(path);
            const unwanted = requested.filter(path => lazy(path)
              || (/\/(items|pagefind|offline)\//.test(path) && !visible.has(path))
              || /\/(sources|categories|tags)\//.test(path));
            const names = await caches.keys();
            const shell = await caches.open(names.find(name => name.includes(':shell-')));
            const stored = (await shell.keys()).map(request => new URL(request.url).pathname);
            done({ unwanted, external: performance.getEntriesByType('resource').filter(entry => /^https?:/.test(entry.name) && new URL(entry.name).origin !== location.origin).map(entry => entry.name),
              transfer: [...performance.getEntriesByType('navigation'), ...performance.getEntriesByType('resource')].reduce((bytes, entry) => bytes + (entry.transferSize || 0), 0),
              bulk: stored.filter(path => lazy(path) || /\/(items|pagefind|offline)\//.test(path)) });
          })().catch(error => done({error: String(error)}));
        "#, vec![]).await?;
        anyhow::ensure!(idle["unwanted"] == serde_json::json!([]), "unexpected idle requests: {idle}");
        anyhow::ensure!(idle["bulk"] == serde_json::json!([]), "unexpected install downloads: {idle}");
        anyhow::ensure!(idle["external"] == serde_json::json!([]), "unexpected external requests: {idle}");
        let served = fixture.served_bytes.load(std::sync::atomic::Ordering::Relaxed);
        eprintln!("default first visit: {served} uncompressed HTTP response bytes including worker requests; {} browser timing transfer bytes", idle["transfer"]);
        anyhow::ensure!(served <= 1024 * 1024, "default fixture visit exceeded 1 MiB including worker installation: {served} bytes");
        screenshot(&client, "default-first-visit").await?;
        client.find(Locator::Css("#q")).await?.send_keys("article").await?;
        wait_for(&client, "document.querySelector('#list')?.getAttribute('aria-busy') === 'false' && document.querySelectorAll('#list .row').length > 0").await?;
        anyhow::ensure!(client.execute("return performance.getEntriesByType('resource').some(entry => entry.name.includes('/pagefind/'))", vec![]).await? == true,
            "search must load its index on intent");
        Ok(())
    }.await;
    report_failure(&client, "default-visit", &result).await;
    finish(client, result).await
}
