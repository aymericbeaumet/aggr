//! Feed rows: selection, hover, activation and keyboard shortcuts on the list pages.

use anyhow::{Context as _, Result};
use fantoccini::{Client, Locator};
use serde_json::json;

use crate::harness::{
    Fixture, browser_client, catch_panics, emulate, finish, key, phone_session, report_failure,
    wait_booted_with, wait_for,
};

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn feed_rows_select_hover_and_activate() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::new()?;
    let client = browser_client().await?;
    let result = catch_panics(feed_row_contracts(&client, &fixture)).await;
    report_failure(&client, "feed-rows", &result).await;
    finish(client, result).await
}

async fn feed_row_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    phone_session(client).await?;
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
        wait_booted_with(client, "!!document.querySelector('.row [data-row-open]')").await?;
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
          const box = node => node.getBoundingClientRect();
          const at = (x, y) => document.elementFromPoint(x, y);
          const middle = node => { const r = box(node); return at(r.left + r.width / 2, r.top + r.height / 2); };
          const before = {row:box(row).left, title:box(title).left, rank:box(rank).left, background:getComputedStyle(row).backgroundColor};
          // The row is one stretched link, so empty row space belongs to the article's own anchor.
          // Modifier and middle clicks are then the browser's, not something script has to forward.
          const rowBox = box(row);
          const blank = [at(rowBox.left + 2, rowBox.top + 2)];
          if (getComputedStyle(rank).display !== 'none') blank.push(middle(rank));
          const category = row.querySelector('.category a'), original = row.querySelector('.u-bookmark-of');
          const reachable = [category, original].every(link => middle(link)?.closest('a') === link);
          row.classList.add('is-selected');
          const after = {row:box(row).left, title:box(title).left, rank:box(rank).left, background:getComputedStyle(row).backgroundColor};
          const selected = getComputedStyle(row, '::before');
          const marker = {width:selected.width, opacity:selected.opacity, transition:selected.transitionDuration, display:selected.display};
          const field = original.parentElement;
          const linkRect = box(original), fieldRect = box(field);
          const rule = getComputedStyle(field, '::before');
          const separator = {balanced:rule.marginLeft === rule.marginRight, content:rule.content};
          const hit = at((fieldRect.left + linkRect.left) / 2, linkRect.top + linkRect.height / 2);
          row.classList.remove('is-selected');
          return {stretched:blank.every(probe => probe === title), reachable, before, after, marker, separator:{outsideLink:!hit.closest('a'), ...separator}, article:title.href, category:category.href};
        "#, vec![]).await?;
        assert_eq!(
            row_actions["stretched"], true,
            "empty row space opens the article: {row_actions}"
        );
        assert_eq!(
            row_actions["reachable"], true,
            "metadata links stay above the stretched link: {row_actions}"
        );
        assert_eq!(
            row_actions["before"], row_actions["after"],
            "selecting a row must keep its background and row, title, and rank positions unchanged"
        );
        assert_eq!(
            row_actions["marker"],
            json!({"width":"3px","opacity":"1","transition":"0s","display":if mobile {"none"} else {"block"}}),
            "the instant 3px selection marker is shown only on desktop: {row_actions}"
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
        wait_booted_with(client, "!!document.querySelector('.row .rank')").await?;
        // The driver refuses a click the stretched link would intercept, which is the contract
        // above; here the link itself stands in for every point of the row it covers.
        client
            .find(Locator::Css(".row [data-row-open]"))
            .await?
            .click()
            .await?;
        wait_for(
            client,
            &format!("location.href === {}", row_actions["article"]),
        )
        .await?;
    }
    Ok(())
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
            wait_booted_with(&client, "!!document.querySelector('.search-command') && !!document.querySelector('.row.is-selected')").await?;
            if search {
                client.find(Locator::Css("#q")).await?.send_keys("source:example").await?;
                wait_for(&client, "!!document.querySelector('.search-results .row.is-selected')").await?;
                key(&client, "Escape").await?;
            }
            // The arrows walk the list wherever j and k do, so nobody needs the vim keys to read.
            const CURSOR: &str = "const root=document.querySelector('.search-results:not([hidden])') || document.querySelector('[data-static-feed]');return [...root.querySelectorAll('.row')].findIndex(row=>row.classList.contains('is-selected'))";
            let start = client.execute(CURSOR, vec![]).await?;
            key(&client, "\u{e015}").await?;
            let stepped = client.execute(CURSOR, vec![]).await?;
            key(&client, "\u{e013}").await?;
            let back = client.execute(CURSOR, vec![]).await?;
            anyhow::ensure!(
                stepped.as_i64() == start.as_i64().map(|index| index + 1) && back == start,
                "ArrowDown and ArrowUp walk the list like j and k: {start} -> {stepped} -> {back}"
            );
            key(&client, "j").await?;
            let targets = client.execute(r#"
              const root=document.querySelector('.search-results:not([hidden])') || document.querySelector('[data-static-feed]');
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
    report_failure(&client, "external-shortcuts", &result).await;
    finish(client, result).await
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
            wait_booted_with(&client, "!!document.querySelector('.search-command') && !!document.querySelector('.row.is-selected')").await?;
            if search {
                client.find(Locator::Css("#q")).await?.send_keys("source:example").await?;
                wait_for(&client, "!!document.querySelector('.search-results .row.is-selected')").await?;
                key(&client, "Escape").await?;
            }
            for (keys, edge) in [("G", "last"), ("gg", "first")] {
                key(&client, keys).await?;
                let state = client.execute("const root=document.querySelector('.search-results:not([hidden])') || document.querySelector('[data-static-feed]');const rows=[...root.querySelectorAll('.row:not([hidden])')];const expected=arguments[0]==='first'?rows[0]:rows.at(-1);const box=expected.getBoundingClientRect();const header=document.querySelector('.top').getBoundingClientRect().bottom;const tabs=document.querySelector('.mobile-tabs')?.getBoundingClientRect();const floor=tabs&&tabs.height>0?tabs.top:innerHeight;return {count:rows.length,selected:expected.classList.contains('is-selected'),focused:expected.querySelector('[data-row-open]')===document.activeElement,visible:box.top>=header-1&&box.bottom<=floor+1}", vec![json!(edge)]).await?;
                anyhow::ensure!(state["count"].as_u64().unwrap_or(0)>1 && state["selected"]==true && state["focused"]==true, "{keys} must select and focus the {edge} visible feed row (search={search}): {state}");
                // The cursor is the row, not the scroll offset: the {edge} row has to end up on
                // screen below the sticky header, whatever else the page keeps above or below it.
                anyhow::ensure!(state["visible"]==true, "{keys} must bring the {edge} row into view (search={search}): {state}");
            }
        }
        client.goto(&format!("{}items/example/2026-09-01-story-45/", fixture.base)).await?;
        wait_booted_with(&client, "!!document.querySelector('article.item')").await?;
        client.execute("document.querySelector('.body').style.minHeight='250vh'",vec![]).await?;
        key(&client,"G").await?;
        wait_for(&client,"scrollY>0 && Math.abs(document.documentElement.scrollHeight-innerHeight-scrollY)<2").await?;
        key(&client,"gg").await?;
        wait_for(&client,"scrollY===0").await?;
        Ok(())
    }.await;
    report_failure(&client, "boundary-shortcuts", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn shift_modifier_highlights_hovered_links() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        client.goto(&fixture.base).await?;
        // The feed link is the current section here; hover feedback is about the others.
        let inactive = "document.querySelector('.nav .menu-link:not([aria-current])')";
        wait_booted_with(&client, inactive).await?;
        let point=client.execute(&format!("const r={inactive}.getBoundingClientRect();return {{x:r.left+20,y:r.top+r.height/2}}"),vec![]).await?;
        emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mouseMoved","x":point["x"],"y":point["y"]})).await?;
        let decoration=format!("getComputedStyle({inactive}).textDecorationLine");
        let before=client.execute(&format!("return {{decoration:{decoration},classes:document.documentElement.className,hovered:[...document.querySelectorAll('a:hover')].map(link=>link.className),selected:{inactive}.outerHTML}}"),vec![]).await?;
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
    report_failure(&client, "shift-hover", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn a_new_deployment_reaches_a_feed_that_is_already_open() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        client.goto(&fixture.base).await?;
        wait_booted_with(&client, "document.querySelectorAll('[data-row-open]').length === 3").await?;
        let before = client
            .execute("return document.querySelector('[data-row-open]').href", vec![])
            .await?;
        fixture.deploy_content_update(7)?;
        // Coming back to the window is when a reader expects to be caught up, so that is when the
        // page asks the build what it has published since.
        client
            .execute("window.dispatchEvent(new Event('focus'))", vec![])
            .await?;
        wait_for(
            &client,
            "document.querySelector('[data-row-open]')?.textContent.includes('Freshly delivered article 7')",
        )
        .await?;
        let swapped = client.execute(r#"
          const rows=[...document.querySelectorAll('.row')];
          return {rows:rows.length, selected:rows.filter(row=>row.classList.contains('is-selected')).length,
            dated:!!document.querySelector('.row time[datetime]')?.textContent.trim(),
            previous:rows.some(row=>row.querySelector('[data-row-open]')?.href===arguments[0])};
        "#, vec![before]).await?;
        anyhow::ensure!(
            swapped["selected"] == 1 && swapped["dated"] == true && swapped["previous"] == true,
            "the swapped list keeps one cursor, its dates, and the articles that were already there: {swapped}"
        );
        Ok(())
    }
    .await;
    report_failure(&client, "feed-updates", &result).await;
    finish(client, result).await
}
