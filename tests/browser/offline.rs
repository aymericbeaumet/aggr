//! The service worker: precache contracts, app updates in place, and reading offline.

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

    wait_for(client, "!!navigator.serviceWorker.controller").await?;
    let checks = client
        .execute_async(
            include_str!("../fixtures/service_worker_checks.js"),
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
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn app_update_keeps_reading_position_and_offline_reading() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::new()?;
    let client = browser_client().await?;
    let result = catch_panics(app_update_and_offline_contracts(&client, &fixture)).await;
    report_failure(&client, "app-update-offline", &result).await;
    finish(client, result).await
}

async fn app_update_and_offline_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
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
    // The worker must control the page before an app update can be offered to it.
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
