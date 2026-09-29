//! Contracts restored from the final Svelte reader, exercised through the current browser client.

use anyhow::Result;
use fantoccini::{Client, Locator};
use serde_json::json;

use crate::harness::{
    Fixture, browser_client, catch_panics, emulate, finish, key, report_failure, wait_booted_with,
    wait_for,
};

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn shared_passage_restores_without_opening_the_gesture_toolbar() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = catch_panics(selection_contract(&client, &fixture)).await;
    report_failure(&client, "shared-passage", &result).await;
    finish(client, result).await
}

async fn selection_contract(client: &Client, fixture: &Fixture) -> Result<()> {
    let article = format!(
        "{}items/example/2026-09-01-story-40/#selection=MCw0",
        fixture.base
    );
    client.goto(&article).await?;
    wait_booted_with(
        client,
        "document.documentElement.dataset.readerReady === 'true'",
    )
    .await?;
    wait_for(client, "getSelection().toString().trim().length > 0").await?;
    assert_eq!(
        client
            .execute(
                "return document.querySelector('.selection-share').hidden",
                vec![]
            )
            .await?,
        true
    );
    let selected = client
        .execute("return getSelection().toString()", vec![])
        .await?;
    assert!(
        selected
            .as_str()
            .is_some_and(|text| text.contains("paragraph"))
    );
    client.execute("document.dispatchEvent(new PointerEvent('pointerdown', {bubbles:true}));document.dispatchEvent(new Event('selectionchange'))", vec![]).await?;
    wait_for(client, "!document.querySelector('.selection-share').hidden").await?;
    assert_eq!(client.current_url().await?.as_str(), article);
    client
        .execute("getSelection().removeAllRanges()", vec![])
        .await?;
    wait_for(client, "document.querySelector('.selection-share').hidden").await?;
    assert_eq!(
        client.current_url().await?.as_str(),
        article,
        "selection changes keep the address the reader arrived with"
    );
    client.find(Locator::Css(".brand")).await?.click().await?;
    wait_for(client, "location.pathname === '/reader/'").await?;
    assert_eq!(
        client
            .execute(
                "return document.querySelectorAll('.selection-share').length",
                vec![]
            )
            .await?,
        0
    );
    key(client, "g").await?;
    key(client, "l").await?;
    wait_for(client, "location.pathname === '/reader/browse/'").await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn article_polling_detects_content_while_a_release_refresh_is_pending() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = catch_panics(polling_contract(&client, &fixture)).await;
    report_failure(&client, "article-update-polling", &result).await;
    finish(client, result).await
}

async fn polling_contract(client: &Client, fixture: &Fixture) -> Result<()> {
    // Shorten only the real polling interval. No reconnect, focus or navigation event triggers
    // these checks: an article left open must discover a deployment on its own.
    emulate(client, "Page.addScriptToEvaluateOnNewDocument", json!({"source": r#"
        const interval = window.setInterval;
        window.setInterval = (fn, delay, ...args) => interval(fn, delay === 15000 ? 200 : delay, ...args);
        window.__contentUpdates = [];
        document.addEventListener('aggr:content-update', event => window.__contentUpdates.push(event.detail));
    "#})).await?;
    let article = format!("{}items/example/2026-09-01-story-40/", fixture.base);
    client.goto(&article).await?;
    wait_booted_with(
        client,
        "document.documentElement.dataset.readerReady === 'true'",
    )
    .await?;
    client
        .execute("window.__sameDocument = true;scrollTo(0,300)", vec![])
        .await?;
    let before = client.execute("return scrollY", vec![]).await?;
    let path = fixture.out.join("updates.json");
    let mut update: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    update["app_version"] = json!("new-reader-release");
    std::fs::write(&path, serde_json::to_vec(&update)?)?;
    wait_for(client, "!document.querySelector('#pwa-refresh').hidden").await?;
    update["content_version"] = json!("new-articles-during-release-prompt");
    std::fs::write(&path, serde_json::to_vec(&update)?)?;
    wait_for(client, "window.__contentUpdates.some(update => update.content_version === 'new-articles-during-release-prompt')").await?;
    assert_eq!(
        client
            .execute("return window.__sameDocument", vec![])
            .await?,
        true
    );
    assert_eq!(client.execute("return scrollY", vec![]).await?, before);
    assert_eq!(client.current_url().await?.as_str(), article);
    assert_eq!(
        client
            .execute(
                "return document.querySelector('#pwa-refresh').hidden",
                vec![]
            )
            .await?,
        false
    );
    Ok(())
}
