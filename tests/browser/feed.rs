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
          const before = {row:row.getBoundingClientRect().left, title:title.getBoundingClientRect().left, rank:rank.getBoundingClientRect().left, background:getComputedStyle(row).backgroundColor};
          const forwarded = [];
          const capture = event => {
            forwarded.push({type:event.type, button:event.button, ctrl:event.ctrlKey, shift:event.shiftKey});
            event.preventDefault();
            event.stopPropagation();
          };
          title.addEventListener('click', capture);
          title.addEventListener('auxclick', capture);
          const surface=getComputedStyle(rank).display==='none' ? row : rank;
          surface.dispatchEvent(new MouseEvent('click', {bubbles:true,cancelable:true,ctrlKey:true,shiftKey:true}));
          surface.dispatchEvent(new MouseEvent('auxclick', {bubbles:true,cancelable:true,button:1}));
          const range = document.createRange();
          range.selectNodeContents(title);
          getSelection().removeAllRanges();
          getSelection().addRange(range);
          surface.dispatchEvent(new MouseEvent('click', {bubbles:true,cancelable:true}));
          getSelection().removeAllRanges();
          title.removeEventListener('click', capture);
          title.removeEventListener('auxclick', capture);
          const after = {row:row.getBoundingClientRect().left, title:title.getBoundingClientRect().left, rank:rank.getBoundingClientRect().left, background:getComputedStyle(row).backgroundColor};
          const marker = getComputedStyle(row, '::before');
          const original = row.querySelector('.u-bookmark-of'), field = original.parentElement;
          const linkRect = original.getBoundingClientRect(), fieldRect = field.getBoundingClientRect();
          const separator = getComputedStyle(field, '::before');
          const hit = document.elementFromPoint((fieldRect.left + linkRect.left) / 2, linkRect.top + linkRect.height / 2);
          return {forwarded, before, after, marker:{width:marker.width,opacity:marker.opacity,transition:marker.transitionDuration,display:marker.display}, separator:{outsideLink:!hit.closest('a'),balanced:separator.marginLeft===separator.marginRight,content:separator.content}, article:title.href, category:row.querySelector('.category a').href};
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
        client
            .find(Locator::Css(if mobile {
                ".row [data-row-open]"
            } else {
                ".row .rank"
            }))
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
            key(&client, "j").await?;
            let targets = client.execute(r#"
              const root=document.querySelector('.search-results') || document.querySelector('[data-static-feed]');
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
                let state = client.execute("const root=document.querySelector('.search-results') || document.querySelector('[data-static-feed]');const rows=[...root.querySelectorAll('.row:not([hidden])')];const expected=arguments[0]==='first'?rows[0]:rows.at(-1);return {count:rows.length,selected:expected.classList.contains('is-selected'),focused:expected.querySelector('[data-row-open]')===document.activeElement}", vec![json!(edge)]).await?;
                anyhow::ensure!(state["count"].as_u64().unwrap_or(0)>1 && state["selected"]==true && state["focused"]==true, "{keys} must select and focus the {edge} visible feed row (search={search}): {state}");
                let boundary = if edge == "first" { "scrollY === 0" } else { "Math.abs(document.documentElement.scrollHeight - innerHeight - scrollY) < 2" };
                anyhow::ensure!(client.execute(&format!("return {boundary}"), vec![]).await? == true, "{keys} must also reach the {edge} scroll boundary (search={search})");
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
