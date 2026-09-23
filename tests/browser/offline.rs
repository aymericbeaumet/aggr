//! The service worker: registration, the precached shell, and reading offline what has been read.

use std::sync::atomic::Ordering;

use anyhow::Result;
use fantoccini::{Client, Locator};
use serde_json::{Value, json};

use crate::harness::{
    Fixture, browser_client, catch_panics, emulate, finish, key, phone_session, report_failure,
    screenshot, set_offline, wait_booted, wait_for,
};

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn disabled_shortcuts_service_worker_checks_and_manifest_identity() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::new()?;
    let client = browser_client().await?;
    let result = catch_panics(service_worker_contracts(&client, &fixture)).await;
    report_failure(&client, "service-worker", &result).await;
    finish(client, result).await
}

async fn service_worker_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    phone_session(client).await?;
    // Single-key shortcuts are off here, so `j` must leave the feed cursor alone.
    client.goto(&fixture.base).await?;
    wait_booted(client).await?;
    client
        .execute(
            "localStorage.setItem('aggr:single-key-shortcuts','false')",
            vec![],
        )
        .await?;
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

    // The browser confirms the real registration: it succeeds, its worker takes control, and the
    // precached shell is served with no network.
    wait_for(client, "!!navigator.serviceWorker.controller").await?;
    let registration = client
        .execute_async(
            r#"
      const done = arguments[arguments.length-1];
      navigator.serviceWorker.getRegistration().then(registration => done({
        scope: registration?.scope || null,
        active: registration?.active?.scriptURL || null,
        controller: navigator.serviceWorker.controller?.scriptURL || null
      }), error => done({error: error.message}));
    "#,
            vec![],
        )
        .await?;
    assert_eq!(registration["scope"], json!(fixture.base), "{registration}");
    assert_eq!(
        registration["active"],
        json!(format!("{}sw.js", fixture.base)),
        "{registration}"
    );
    assert_eq!(
        registration["controller"], registration["active"],
        "the registered worker must be the one controlling the page"
    );
    set_offline(client, true).await?;
    client
        .goto(&format!("{}offline.html", fixture.base))
        .await?;
    // The shell is precached, so the page that explains the situation is itself readable offline.
    wait_for(
        client,
        "!navigator.onLine && document.querySelector('.listhead h1')?.textContent === 'Offline'",
    )
    .await?;
    set_offline(client, false).await?;
    wait_for(client, "navigator.onLine").await?;
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
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn what_has_been_read_stays_readable_offline() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::new()?;
    let client = browser_client().await?;
    let result = catch_panics(offline_reading_contracts(&client, &fixture)).await;
    report_failure(&client, "offline-reading", &result).await;
    finish(client, result).await
}

async fn offline_reading_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    phone_session(client).await?;
    emulate(
        client,
        "Emulation.setTouchEmulationEnabled",
        json!({"enabled":false}),
    )
    .await?;
    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":320,"height":844,"deviceScaleFactor":1,"mobile":true}),
    )
    .await?;
    client.goto(&fixture.base).await?;
    wait_booted(client).await?;
    wait_for(client, "!!navigator.serviceWorker.controller").await?;
    client
        .execute("localStorage.setItem('aggr:theme','dark')", vec![])
        .await?;
    emulate(
        client,
        "Emulation.setEmulatedMedia",
        json!({"features":[{"name":"prefers-reduced-motion","value":"reduce"}]}),
    )
    .await?;
    client.goto(&fixture.base).await?;
    assert_eq!(
        client.execute("return typeof window.swup", vec![]).await?,
        "undefined",
        "no navigation framework runs between the reader and the browser"
    );
    screenshot(client, "mobile-dark").await?;
    client
        .find(Locator::Css("[data-row-open]"))
        .await?
        .click()
        .await?;
    wait_for(client, "!!document.querySelector('.body pre')").await?;
    // What has been read is what is kept: the worker caches the pages someone opened, and their
    // content-addressed pictures, and never downloads an archive ahead of them.
    client.execute("window.scrollTo(0,300)", vec![]).await?;
    let read_article = client.current_url().await?;
    wait_for(
        client,
        "document.querySelector('.progressive-image')?.naturalWidth === 640",
    )
    .await?;
    client.goto(&fixture.base).await?;
    wait_for(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3",
    )
    .await?;

    fixture.offline.store(true, Ordering::Relaxed);
    set_offline(client, true).await?;
    wait_for(client, "!navigator.onLine").await?;
    client.goto(read_article.as_str()).await?;
    wait_for(client, "!!document.querySelector('.body pre')").await?;
    wait_for(
        client,
        "document.querySelector('.progressive-image')?.naturalWidth === 640",
    )
    .await?;
    assert_eq!(
        client.execute("return navigator.onLine", vec![]).await?,
        false,
        "the article opened before must read the same way with no network"
    );
    screenshot(client, "mobile-dark").await?;
    // An article nobody opened was never downloaded, and says so instead of failing blankly.
    client
        .goto(&format!(
            "{}items/example/2026-09-01-story-30/",
            fixture.base
        ))
        .await?;
    wait_for(
        client,
        "document.querySelector('.listhead h1')?.textContent === 'Offline'",
    )
    .await?;
    fixture.offline.store(false, Ordering::Relaxed);
    set_offline(client, false).await?;
    Ok(())
}
