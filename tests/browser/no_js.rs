//! The generated pages stay usable when every script is blocked.

use std::sync::atomic::Ordering;

use anyhow::Result;
use fantoccini::{Client, Locator};
use serde_json::json;

use crate::harness::{
    Fixture, browser_client, catch_panics, emulate, finish, report_failure, screenshot, wait_for,
};

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn reader_works_without_javascript() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::new()?;
    fixture.scripts_blocked.store(true, Ordering::Relaxed);
    let client = browser_client().await?;
    let result = catch_panics(no_javascript_contracts(&client, &fixture)).await;
    report_failure(&client, "no-javascript", &result).await;
    finish(client, result).await
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
            "return {base:new URL(document.getElementById('aggr-page').dataset.root,location.href).href,canonical:!!document.querySelector('link[rel=canonical]'),schema:!!document.querySelector('script[type=\"application/ld+json\"]')}",
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
