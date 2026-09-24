//! The phone surface: tab bar, compact rows, shortcut help and route handling.

use anyhow::{Context as _, Result};
use fantoccini::{Client, Locator};
use serde_json::{Value, json};

use crate::harness::{
    Fixture, browser_client, catch_panics, emulate, escape_search, finish, key, phone_session,
    report_failure, screenshot, wait_booted_with, wait_for,
};

/// The phone feed measurement shared by the layout, density and responsive header contracts.
pub(crate) async fn mobile_feed_layout(client: &Client) -> Result<Value> {
    Ok(client.execute(r#"
      const nav = document.querySelector('.nav');
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
      return {count:nav.querySelectorAll('a').length, visible:[...nav.querySelectorAll('a')].filter(link => link.getBoundingClientRect().height > 0).length===2 && [...document.querySelectorAll('.mobile-tabs a')].every(link=>link.getBoundingClientRect().height>=44),
        labels:[...nav.querySelectorAll('.menu-link')].map(link=>link.textContent.trim()),
        separators:[...nav.querySelectorAll('.nav-primary .nav-separator')].map(span=>({text:span.textContent,hidden:span.getAttribute('aria-hidden')})),
        searchIcon:!!nav.querySelector('[data-search-open]'),
        searchBelowNav:document.querySelector('[data-search-root]').getBoundingClientRect().top >= top.getBoundingClientRect().bottom,
        searchAboveFeed:document.querySelector('[data-search-root]').getBoundingClientRect().bottom <= document.querySelector('[data-static-feed]').getBoundingClientRect().top,
        flat:!document.querySelector('#site-menu, .feed-tabs'),
        overflow:document.documentElement.scrollWidth > document.documentElement.clientWidth + 1,
        reload:!!document.querySelector('#refresh-page'), density:document.documentElement.dataset.density,
        configVisible:getComputedStyle(config).display !== 'none',
        configRight:innerWidth-config.getBoundingClientRect().right,
        selected:document.querySelectorAll('.row.is-selected').length,
        brandLeft:brand.getBoundingClientRect().left, rankDisplay:getComputedStyle(rank).display,
        rankWidth:rank.getBoundingClientRect().width,
        titleLeft:title.getBoundingClientRect().left,metaLeft:meta.getBoundingClientRect().left,
        metadataInset:parseFloat(getComputedStyle(row.querySelector(".meta")).paddingLeft),
        header:{height:top.getBoundingClientRect().height,position:topStyle.position,
          background:topStyle.backgroundColor,navMinHeight:topNavStyle.minHeight,
          navPaddingLeft:topNavStyle.paddingLeft,navPaddingRight:topNavStyle.paddingRight,
          brandMarginLeft:brandLinkStyle.marginLeft,brandMarginRight:brandLinkStyle.marginRight,
          configFontSize:configStyle.fontSize,configPaddingLeft:configStyle.paddingLeft,
          configPaddingRight:configStyle.paddingRight},
        rowHeight:row.getBoundingClientRect().height,
        rowInset:title.getBoundingClientRect().left-row.getBoundingClientRect().left,
        rowInlinePadding:parseFloat(getComputedStyle(row).paddingLeft),
        rowPadding:parseFloat(getComputedStyle(row).paddingTop)};
    "#, vec![]).await?)
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn mobile_layout_shortcut_help_routes_and_scroll_shortcuts() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::new()?;
    let client = browser_client().await?;
    let result = catch_panics(mobile_layout_contracts(&client, &fixture)).await;
    report_failure(&client, "mobile-layout", &result).await;
    finish(client, result).await
}

async fn mobile_layout_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    phone_session(client).await?;
    client.goto(&fixture.base).await?;
    wait_for(
        client,
        "document.documentElement.dataset.aggrReady === 'true'",
    )
    .await?;
    let feed_page_size = client
        .execute(
            r#"
      const list = document.querySelector('.rows:not(.search-results)');
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
    wait_booted_with(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3",
    )
    .await?;
    let layout = mobile_feed_layout(client).await?;
    assert_eq!(layout["count"], 5);
    assert_eq!(layout["labels"], json!(["feed", "browse", "preferences"]));
    assert_eq!(
        layout["separators"],
        json!([{ "text": "|", "hidden": "true" }, { "text": "|", "hidden": "true" }])
    );
    assert_eq!(layout["searchIcon"], false);
    assert_eq!(layout["flat"], true);
    assert_eq!(layout["searchBelowNav"], true);
    assert_eq!(layout["searchAboveFeed"], true);
    assert_eq!(layout["visible"], true);
    assert_eq!(layout["overflow"], false);
    assert_eq!(layout["reload"], false);
    assert_eq!(layout["density"], "compact");
    assert_eq!(layout["configVisible"], true);
    assert!(
        layout["configRight"].as_f64().unwrap_or_default() <= 17.0,
        "Preferences and config should stay at the mobile header's right edge: {layout}"
    );
    assert_eq!(layout["selected"], 1, "the first feed row starts selected");
    assert_eq!(layout["rankDisplay"], "none", "mobile ranks are hidden");
    assert_eq!(layout["rankWidth"], 0, "hidden ranks reserve no width");
    assert_eq!(
        layout["metadataInset"], 0,
        "metadata reclaims the rank column"
    );
    assert!(
        (layout["metaLeft"].as_f64().unwrap_or_default()
            - layout["titleLeft"].as_f64().unwrap_or_default())
        .abs()
            <= 1.0,
        "mobile metadata should share the title text axis: {layout}"
    );
    let compact_row_height = layout["rowHeight"].as_f64().context("compact row height")?;
    assert!(
        compact_row_height <= 140.0,
        "compact mobile row should stay dense: {layout}"
    );
    assert!(
        (layout["rowInset"].as_f64().unwrap_or_default()
            - layout["rowInlinePadding"].as_f64().unwrap_or_default())
        .abs()
            <= 1.0,
        "mobile titles reclaim the rank column and retain only the row inset: {layout}"
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
    // Tapping a row is an ordinary navigation the browser has already been told to prepare.
    assert_eq!(
        client
            .execute(
                "return document.querySelector('.rows .row [data-row-open]').matches(JSON.parse(document.querySelector('script[type=speculationrules]').textContent).prerender[0].where.selector_matches)",
                vec![]
            )
            .await?,
        true,
        "the row a tap opens is one the speculation rules name"
    );
    screenshot(client, "mobile-feed").await?;
    client
        .execute("document.querySelector('.brand').focus()", vec![])
        .await?;
    // `/` is the search key wherever the reader is, including with the keyboard on the brand.
    key(client, "/").await?;
    wait_for(client, "document.activeElement?.id === 'q'").await?;
    anyhow::ensure!(
        client
            .execute("return document.querySelector('#q').value === ''", vec![])
            .await?
            == true,
        "opening search leaves the field empty rather than typing the key into it"
    );
    escape_search(client).await?;
    key(client, "\u{e009}k\u{e000}").await?;
    wait_for(
        client,
        "document.activeElement?.id === 'q' && document.body.dataset.kind === 'river'",
    )
    .await?;
    assert_eq!(
        client.execute("return location.pathname", vec![]).await?,
        "/reader/",
        "the search shortcut focuses the feed input without opening the selected article"
    );
    key(client, "Escape").await?;
    assert_eq!(
        client
            .execute(
                "return document.querySelectorAll('.row .domain').length",
                vec![]
            )
            .await?,
        3,
        "every feed row should show its source below the title"
    );

    for (route, path, title) in [
        ("browse/", "/reader/browse/", "browse | Reading room | aggr"),
        (
            "preferences/",
            "/reader/preferences/",
            "preferences | Reading room | aggr",
        ),
        ("", "/reader/", "Reading room | aggr"),
    ] {
        let selector = if route.is_empty() {
            ".brand".to_string()
        } else {
            format!(".mobile-tabs a[data-route='{route}']")
        };
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
                .execute(
                    "return new URL(window.AGGR.base,location.href).href",
                    vec![]
                )
                .await?,
            fixture.base,
            "the persistent document base must keep every mobile route site-relative"
        );
        assert_eq!(client.title().await?, title, "{path} document title");
    }

    key(client, "\u{e03d}k\u{e000}").await?;
    wait_for(
        client,
        "location.pathname === '/reader/' && document.activeElement?.id === 'q'",
    )
    .await?;
    for (directory, kind, value) in [
        ("browse", "", ""),
        ("sources/publisher.invalid", "source", "publisher.invalid"),
        ("tags/reading", "tag", "reading"),
        ("categories/engineering", "category", "engineering"),
    ] {
        let destination = format!("{}{directory}/", fixture.base);
        client.goto(&destination).await?;
        wait_for(
            client,
            "document.documentElement.dataset.aggrReady === 'true'",
        )
        .await?;
        let scopes = client.execute("const root=document.querySelector('[data-search-root]');return root ? {kind:root.dataset.scopeKind,value:root.dataset.scopeValue} : null", vec![]).await?;
        if kind.is_empty() {
            assert_eq!(
                scopes,
                Value::Null,
                "directory pages use the global search shortcut"
            );
        } else {
            assert_eq!(scopes, json!({"kind":kind,"value":value}));
        }
        key(client, "\u{e009}k\u{e000}").await?;
        wait_for(client, "document.activeElement?.id === 'q'").await?;
        let path = if kind.is_empty() {
            "/reader/".to_string()
        } else {
            format!("/reader/{directory}/")
        };
        assert_eq!(
            client.current_url().await?.path(),
            path,
            "search should retain the current archive scope and route"
        );
        assert_eq!(
            client
                .execute("return document.querySelectorAll('#q').length", vec![])
                .await?,
            1
        );
    }
    for route in ["", "browse/", "?q=article", "preferences/"] {
        client.goto(&format!("{}{route}", fixture.base)).await?;
        wait_for(
            client,
            "document.documentElement.dataset.aggrReady === 'true'",
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
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn mobile_tabs_and_tab_navigation() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_base_path(false, "reader/")?;
    let client = browser_client().await?;
    let result = async {
        for (width, height) in [(390, 844), (360, 780)] {
            emulate(&client, "Emulation.setDeviceMetricsOverride", json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":true})).await?;
            client.goto(&fixture.base).await?;
            wait_booted_with(&client, "!!document.querySelector('.search-command')").await?;
            let layout = client.execute(r#"
              const bar=document.querySelector('.mobile-tabs'),r=bar.getBoundingClientRect();
              const visible=element=>!!element.getClientRects().length&&getComputedStyle(element).visibility!=='hidden';
              return {bottom:r.bottom,height:r.height,viewport:innerHeight,position:getComputedStyle(bar).position,
                top:[...document.querySelectorAll('.nav a')].filter(visible).map(a=>a.textContent.trim()),
                tabs:[...bar.querySelectorAll('a')].filter(visible).map(a=>({label:a.textContent.trim(),height:a.getBoundingClientRect().height,current:a.getAttribute('aria-current'),iconTop:a.querySelector('svg').getBoundingClientRect().top,iconHeight:a.querySelector('svg').getBoundingClientRect().height,labelTop:a.querySelector('span').getBoundingClientRect().top,radius:parseFloat(getComputedStyle(a).borderRadius),background:getComputedStyle(a).backgroundColor})),
                surface:{left:parseFloat(getComputedStyle(bar,'::before').left),right:parseFloat(getComputedStyle(bar,'::before').right),radius:parseFloat(getComputedStyle(bar,'::before').borderRadius)},
                padding:parseFloat(getComputedStyle(document.querySelector('.main')).paddingBottom),footer:visible(document.querySelector('.footer'))};
            "#,vec![]).await?;
            anyhow::ensure!(layout["position"]=="fixed" && (layout["bottom"].as_f64().unwrap()-layout["viewport"].as_f64().unwrap()).abs()<1.0,"tabs meet the viewport bottom: {layout}");
            anyhow::ensure!(layout["top"].as_array().unwrap().len()==2 && layout["top"][1]=="aggr.toml ↗" && layout["footer"]==false,"mobile header has only brand and config: {layout}");
            let tabs=layout["tabs"].as_array().unwrap();
            anyhow::ensure!(tabs.iter().map(|tab|tab["label"].as_str().unwrap()).collect::<Vec<_>>()==["feed","browse","preferences"] && tabs.iter().all(|tab|tab["height"].as_f64().unwrap()>=44.0) && tabs[0]["current"]=="page","three accessible tabs with an active feed: {layout}");
            anyhow::ensure!(layout["height"]==68 && layout["surface"]["left"]==16 && layout["surface"]["right"]==16 && layout["surface"]["radius"].as_f64().unwrap()>=30.0,"floating surface stays compact and inset from both screen edges: {layout}");
            anyhow::ensure!(tabs.iter().all(|tab|tab["iconTop"]==tabs[0]["iconTop"] && tab["iconHeight"]==24 && tab["labelTop"]==tabs[0]["labelTop"] && tab["radius"].as_f64().unwrap()>=26.0) && tabs[0]["background"]!="rgba(0, 0, 0, 0)","icons and labels share exact vertical tracks and the active tab has a rounded pill: {layout}");
            anyhow::ensure!((layout["padding"].as_f64().unwrap()-layout["height"].as_f64().unwrap()-12.0).abs()<1.0,"content clears the measured bar with one compact gap: {layout}");
            screenshot(&client, &format!("mobile-tabs-{width}")).await?;
            client.execute("scrollTo(0,document.documentElement.scrollHeight);document.documentElement.style.setProperty('--mobile-safe-area','32px')",vec![]).await?;
            // The reserved space is a calc over the safe area, so it is only a number once it has
            // been resolved against something that uses it.
            wait_for(&client,"Math.abs(parseFloat(getComputedStyle(document.querySelector('.main')).paddingBottom)-document.querySelector('.mobile-tabs').getBoundingClientRect().height-12)<1").await?;
            let inset=client.execute(r#"
              const bar=document.querySelector('.mobile-tabs'),r=bar.getBoundingClientRect();
              return {bottom:r.bottom,viewport:innerHeight,height:r.height,padding:parseFloat(getComputedStyle(document.querySelector('.main')).paddingBottom),safe:parseFloat(getComputedStyle(bar).paddingBottom)};
            "#,vec![]).await?;
            anyhow::ensure!(inset["safe"]==44 && (inset["height"].as_f64().unwrap()-layout["height"].as_f64().unwrap()-32.0).abs()<1.0 && (inset["bottom"].as_f64().unwrap()-inset["viewport"].as_f64().unwrap()).abs()<1.0,"scrolling keeps the bar docked and safe area is counted once: {inset}");
            anyhow::ensure!((inset["padding"].as_f64().unwrap()-inset["height"].as_f64().unwrap()-12.0).abs()<1.0,"home-indicator padding is not duplicated in content: {inset}");
            let keyboard=client.execute_async(r#"
              const done=arguments[arguments.length-1],bar=document.querySelector('.mobile-tabs'),input=document.querySelector('#q'),viewport=window.visualViewport;
              const height=bar.getBoundingClientRect().height,padding=getComputedStyle(document.querySelector('.main')).paddingBottom;
              input.focus();
              Object.defineProperty(viewport,'height',{configurable:true,value:innerHeight-300});
              viewport.dispatchEvent(new Event('resize'));
              const hidden=getComputedStyle(bar).visibility==='hidden';
              input.blur();
              delete viewport.height;
              queueMicrotask(()=>done({hidden,restored:getComputedStyle(bar).visibility==='visible',stable:bar.getBoundingClientRect().height===height&&getComputedStyle(document.querySelector('.main')).paddingBottom===padding}));
            "#,vec![]).await?;
            anyhow::ensure!(keyboard==json!({"hidden":true,"restored":true,"stable":true}),"keyboard hides controls without shifting content or floating above keys: {keyboard}");
            // Reselecting the tab of the page already open is how a reader gets back to the top
            // of it, and it leaves the search field alone on the way.
            client.execute("document.querySelector('#q').focus();document.querySelector('.main').style.minHeight='1600px';scrollTo(0,200)",vec![]).await?;
            client.find(Locator::Css(r#".mobile-tabs a[data-route=""]"#)).await?.click().await?;
            wait_booted_with(&client,"document.body.dataset.kind==='river' && scrollY===0").await?;
            let reselected=client.execute(r#"
              const feed=document.querySelector('.mobile-tabs a[data-route=""]');
              document.querySelector('.main').style.removeProperty('min-height');
              return {top:scrollY,editing:document.activeElement===document.querySelector('#q'),href:location.href,expected:feed.href,current:feed.getAttribute('aria-current')};
            "#,vec![]).await?;
            anyhow::ensure!(reselected["top"]==0 && reselected["editing"]==false && reselected["href"]==reselected["expected"] && reselected["current"]=="page","reselecting Feed returns to its top without opening search: {reselected}");
            let tab_point=client.execute(r#"const r=document.querySelector('.mobile-tabs a[data-route="browse/"]').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}"#,vec![]).await?;
            let before_press=client.execute(r#"const tab=document.querySelector('.mobile-tabs a[data-route="browse/"]');const r=tab.getBoundingClientRect();return {background:getComputedStyle(tab).backgroundColor,top:r.top,height:r.height}"#,vec![]).await?;
            emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mousePressed","button":"left","buttons":1,"clickCount":1,"x":tab_point["x"],"y":tab_point["y"]})).await?;
            let pressed=client.execute(r#"const tab=document.querySelector('.mobile-tabs a[data-route="browse/"]');const r=tab.getBoundingClientRect();return {background:getComputedStyle(tab).backgroundColor,top:r.top,height:r.height,transition:getComputedStyle(tab).transitionDuration,opacity:getComputedStyle(tab).opacity}"#,vec![]).await?;
            emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mouseReleased","button":"left","buttons":0,"clickCount":1,"x":1,"y":1})).await?;
            anyhow::ensure!(pressed["background"]!=before_press["background"] && pressed["top"]==before_press["top"] && pressed["height"]==before_press["height"] && pressed["transition"]=="0s" && pressed["opacity"]=="1","tabs give immediate pressed feedback without fading labels or shifting layout: {pressed}");
            // Each tab is an ordinary link to its own page, and exactly one of them is current.
            for route in ["browse/","preferences/",""] {
                client.find(Locator::Css(&format!(".mobile-tabs a[data-route=\"{route}\"]"))).await?.click().await?;
                wait_booted_with(&client,&format!("location.pathname===new URL('{route}',new URL(window.AGGR.base,location.href)).pathname && document.querySelectorAll('.mobile-tabs a[aria-current]').length===1 && document.querySelector('.mobile-tabs a[aria-current]')?.dataset.route==='{route}'")).await?;
                if route=="browse/" {
                    anyhow::ensure!(client.execute("return ['categories','sources','tags'].every(kind=>!!document.querySelector('.browse-group-'+kind+' .browse-entry-link'))",vec![]).await?==true,"Browse must expose every populated directory");
                }
            }
            client.find(Locator::Css("#q")).await?.click().await?;
            wait_for(&client,"document.activeElement?.id==='q' && document.querySelector('.mobile-tabs [aria-current]')?.hasAttribute('data-feed-action')").await?;
            client.find(Locator::Css("#q")).await?.send_keys("category:engineering").await?;
            wait_for(&client,"new URL(location.href).searchParams.get('q')==='category:engineering'").await?;
            let focused=client.execute(r#"
              const input=document.querySelector('#q');input.setSelectionRange(9,12);
              return {query:input.value,selection:[input.selectionStart,input.selectionEnd],focused:document.activeElement===input,active:document.querySelectorAll('.mobile-tabs [aria-current]').length,searchTab:!!document.querySelector('.mobile-tabs [data-search-action]')};
            "#,vec![]).await?;
            anyhow::ensure!(focused==json!({"query":"category:engineering","selection":[9,12],"focused":true,"active":1,"searchTab":false}),"the persistent field preserves query and caret while Feed remains the single active tab: {focused}");
            escape_search(&client).await?;
            wait_for(&client,"document.activeElement?.id!=='q' && document.querySelector('.mobile-tabs [aria-current]')?.hasAttribute('data-feed-action')").await?;
            client.execute("const input=document.querySelector('#q');input.value='';input.dispatchEvent(new Event('input',{bubbles:true}));input.blur()",vec![]).await?;
            wait_for(&client,"!new URL(location.href).searchParams.has('q') && document.querySelector('.mobile-tabs [aria-current]')?.hasAttribute('data-feed-action')").await?;
            client.find(Locator::Css(".mobile-tabs [data-route='browse/']")).await?.click().await?;
            wait_booted_with(&client,"document.body.dataset.kind==='browse'").await?;
            key(&client,"/").await?;
            wait_for(&client,"location.pathname==='/reader/' && document.activeElement?.id==='q' && document.querySelector('.mobile-tabs [aria-current]')?.hasAttribute('data-feed-action')").await?;
        }
        Ok(())
    }.await;
    report_failure(&client, "mobile-tabs", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn mobile_feed_rows_keep_metadata_readable_without_thumbnail_indentation() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_base_path(false, "reader/")?;
    let client = browser_client().await?;
    let result = async {
        for width in [390, 320] {
            emulate(&client, "Emulation.setDeviceMetricsOverride", json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":true})).await?;
            client.goto(&fixture.base).await?;
            wait_booted_with(&client,"!!document.querySelector('.row .preview-media')").await?;
            let layout=client.execute(r#"
              const rows=[...document.querySelectorAll('.row')],row=rows[0],copy=row.querySelector('.row-content'),title=row.querySelector('.title'),meta=row.querySelector('.meta'),preview=row.querySelector('.preview-media');
              const source=meta.querySelector('.meta-field:has(.domain)'),fields=[...meta.children];
              const textOnly=rows.find(row=>!row.querySelector('.preview-media'));
              const box=node=>node.getBoundingClientRect();
              return {overflow:document.documentElement.scrollWidth>innerWidth,
                rank:rows.every(row=>getComputedStyle(row.querySelector('.rank')).display==='none'),
                metadataFullWidth:Math.abs(box(meta).width-box(copy).width)<1,
                sharedAxis:Math.abs(box(meta).left-box(title).left)<1,
                sourceSeparate:fields.slice(1).every(field=>box(field).top>=box(source).bottom-1),
                fieldsVisible:fields.every(field=>box(field).height>0 && getComputedStyle(field).visibility!=='hidden'),
                separators:fields.every(field=>getComputedStyle(field,'::before').content==='none'),
                preview:{width:box(preview).width,height:box(preview).height,top:box(preview).top-title.getBoundingClientRect().top,right:box(preview).right-box(copy).right},
                textOnlyMinHeight:getComputedStyle(textOnly.querySelector('.row-content')).minHeight,
                textOnlyMetadataGap:box(textOnly.querySelector('.meta')).top-box(textOnly.querySelector('.title')).bottom,
                rowTargets:rows.every(row=>box(row).height>=44),
                padding:parseFloat(getComputedStyle(row).paddingTop),
                titleSize:parseFloat(getComputedStyle(title).fontSize),
                metaSize:parseFloat(getComputedStyle(meta).fontSize)};
            "#,vec![]).await?;
            anyhow::ensure!(layout["overflow"]==false && layout["rank"]==true && layout["metadataFullWidth"]==true && layout["sharedAxis"]==true,"mobile text uses the full row width without a rank or preview column below the title: {layout}");
            anyhow::ensure!(layout["sourceSeparate"]==true && layout["fieldsVisible"]==true && layout["separators"]==true,"source and secondary metadata form a clear hierarchy without losing fields or wrapping leading dots: {layout}");
            anyhow::ensure!(layout["preview"]==json!({"width":48,"height":48,"top":0,"right":0}) && layout["textOnlyMinHeight"]=="0px","previews reserve a stable square only where present: {layout}");
            anyhow::ensure!(layout["textOnlyMetadataGap"].as_f64().unwrap()<=6.0,"text-only rows place source metadata directly below the title: {layout}");
            anyhow::ensure!(layout["rowTargets"]==true && layout["padding"].as_f64().unwrap()>=10.0 && layout["titleSize"].as_f64().unwrap()>=16.0 && layout["metaSize"].as_f64().unwrap()>=13.0,"mobile rows retain readable text and a comfortable full-row target: {layout}");
            let hidden=client.execute("document.documentElement.dataset.thumbnails='hide';const row=document.querySelector('.row'),full=row.querySelector('.row-content').getBoundingClientRect().width,title=row.querySelector('.title').getBoundingClientRect().width;const hidden=getComputedStyle(row.querySelector('.preview-media')).display==='none';delete document.documentElement.dataset.thumbnails;return {hidden,reclaimed:Math.abs(full-title)<1}",vec![]).await?;
            anyhow::ensure!(hidden==json!({"hidden":true,"reclaimed":true}),"hiding previews also removes their column and gap: {hidden}");
            screenshot(&client,&format!("mobile-feed-ergonomics-{width}")).await?;
            let target=client.execute("const row=document.querySelector('.row');const box=row.getBoundingClientRect();return {x:box.left+4,y:box.bottom-4,url:row.querySelector('[data-row-open]').href}",vec![]).await?;
            emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mousePressed","button":"left","buttons":1,"clickCount":1,"x":target["x"],"y":target["y"]})).await?;
            emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mouseReleased","button":"left","buttons":0,"clickCount":1,"x":target["x"],"y":target["y"]})).await?;
            wait_for(&client,&format!("location.href==={}",target["url"])).await?;
        }
        Ok(())
    }.await;
    report_failure(&client, "mobile-feed-ergonomics", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn mobile_search_pagination_uses_compact_inline_controls() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        emulate(&client, "Page.addScriptToEvaluateOnNewDocument", json!({"source":"localStorage.setItem('aggr:feed-page-size','10')"})).await?;
        for width in [390, 320] {
            emulate(&client, "Emulation.setDeviceMetricsOverride", json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":true})).await?;
            client.goto(&format!("{}?q=source:%22publisher.invalid%22",fixture.base)).await?;
            wait_for(&client,"!!document.querySelector('[data-search-results] [data-search-page-next]:not([hidden])')").await?;
            let state=client.execute(r#"
              const pager=document.querySelector('[data-search-results] .pager');
              const next=pager.querySelector('[data-search-page-next]');
              const status=pager.querySelector('[data-search-page-status]');
              const n=next.getBoundingClientRect(), s=status.getBoundingClientRect();
              const style=getComputedStyle(next);
              return {inline:Math.abs((n.top+n.bottom)/2-(s.top+s.bottom)/2)<2 && n.left>=s.right,
                compact:n.width<100, touch:n.height>=44,
                plain:style.borderTopWidth==='0px'&&style.backgroundColor==='rgba(0, 0, 0, 0)'};
            "#,vec![]).await?;
            anyhow::ensure!(state==json!({"inline":true,"compact":true,"touch":true,"plain":true}),"pagination stays compact and tappable at {width}px: {state}");
            client.execute("document.querySelector('[data-search-results] .pager').scrollIntoView({block:'center'})",vec![]).await?;
            screenshot(&client,&format!("mobile-search-pagination-{width}")).await?;
        }
        Ok(())
    }.await;
    report_failure(&client, "mobile-search-pagination", &result).await;
    finish(client, result).await
}

async fn touch(client: &Client, phase: &str, point: Option<(f64, f64)>) -> Result<()> {
    let points = point
        .map(|(x, y)| json!([{"x":x,"y":y,"id":1}]))
        .unwrap_or_else(|| json!([]));
    // A touch that opens a page may never be acknowledged: the document it was delivered to is
    // already on its way out. The event still happened, so a missing reply is not a failure.
    let delivered = emulate(
        client,
        "Input.dispatchTouchEvent",
        json!({"type":phase,"touchPoints":points}),
    );
    match tokio::time::timeout(std::time::Duration::from_secs(5), delivered).await {
        Ok(result) => result,
        Err(_) => Ok(()),
    }
}

async fn touch_point(client: &Client, selector: &str) -> Result<(f64, f64)> {
    let point = client
        .execute(
            r#"
      const element=document.querySelector(arguments[0]);
      if(!element)throw Error('missing touch target: '+arguments[0]);
      const box=element.getBoundingClientRect();
      return [box.left+box.width/2,box.top+box.height/2];
    "#,
            vec![json!(selector)],
        )
        .await?;
    Ok((
        point[0].as_f64().context("touch x")?,
        point[1].as_f64().context("touch y")?,
    ))
}

async fn swipe(client: &Client, from: (f64, f64), to: (f64, f64)) -> Result<()> {
    touch(client, "touchStart", Some(from)).await?;
    for step in 1..=8 {
        let fraction = f64::from(step) / 8.0;
        touch(
            client,
            "touchMove",
            Some((
                from.0 + (to.0 - from.0) * fraction,
                from.1 + (to.1 - from.1) * fraction,
            )),
        )
        .await?;
    }
    touch(client, "touchEnd", None).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn mobile_physical_taps_navigate_once_without_delay() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result=async {
        phone_session(&client).await?;
        client.goto(&fixture.base).await?;
        wait_booted_with(&client,"!!document.querySelector('.mobile-tabs a[data-route=\"browse/\"]')").await?;
        let history_before = client.execute("return history.length",vec![]).await?.as_u64().unwrap_or(0);
        for (index,route) in ["browse/","preferences/"].iter().enumerate() {
            let selector=format!(".mobile-tabs a[data-route='{route}']");
            let point=touch_point(&client,&selector).await?;
            let rest=client.execute("return getComputedStyle(document.querySelector(arguments[0])).backgroundColor",vec![json!(selector)]).await?;
            touch(&client,"touchStart",Some(point)).await?;
            let pressed=client.execute("const tab=document.querySelector(arguments[0]);return {background:getComputedStyle(tab).backgroundColor,transition:getComputedStyle(tab).transitionDuration}",vec![json!(selector)]).await?;
            anyhow::ensure!(pressed["background"]!=rest && pressed["transition"]=="0s","touch contact gives immediate visible feedback: {pressed}; resting {rest}");
            // One tap is one navigation: the release goes straight to the page, and the browser
            // is left with one entry for it rather than a second from a re-fired click.
            let released = std::time::Instant::now();
            touch(&client,"touchEnd",None).await?;
            wait_for(&client,&format!("location.pathname===new URL('{route}',new URL(window.AGGR.base,location.href)).pathname && document.querySelector('.mobile-tabs [aria-current]')?.dataset.route==='{route}'")).await?;
            let elapsed = released.elapsed();
            let state=client.execute(r#"
              const tab=document.querySelector('.mobile-tabs [aria-current]');
              return {history:history.length,active:document.querySelectorAll('.mobile-tabs [aria-current]').length,
                touchAction:getComputedStyle(tab).touchAction,decoration:getComputedStyle(tab).textDecorationLine};
            "#,vec![]).await?;
            anyhow::ensure!(state["history"].as_u64().unwrap_or(0)==history_before+index as u64+1 && state["active"]==1,"one physical tap produces one history entry: {state}");
            anyhow::ensure!(elapsed < std::time::Duration::from_secs(2),"navigation starts on release without a double-tap delay: {elapsed:?}");
            anyhow::ensure!(state["touchAction"]=="manipulation" && state["decoration"]=="none","tabs expose immediate touch activation without link decoration: {state}");
        }
        screenshot(&client,"mobile-physical-tap-navigation").await?;
        Ok(())
    }.await;
    report_failure(&client, "mobile-physical-taps", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn mobile_article_gestures_scroll_the_page_and_keys_turn_it() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let archive = fixture.directory.path().join(".aggr/data");
    for (index, body) in [
        (
            28,
            "Ocean currents transport heat between continents. Marine biologists study coastal plankton and tidal habitats. Deep water circulation influences seasonal temperatures and marine life.",
        ),
        (
            29,
            "Ancient manuscripts preserve the history of alphabets. Conservators examine parchment pigments and restore damaged bindings. Museums catalogue writing systems and their cultural origins.",
        ),
        (
            30,
            "Orbital telescopes measure distant stellar spectra. Astronomers track planetary transits and estimate atmospheric composition. Observatory teams compare ultraviolet measurements and calibrate optical instruments.",
        ),
    ] {
        let path = archive.join(format!(
            "items/example/2026/09/2026-09-01-story-{index:02}.md"
        ));
        let original = std::fs::read_to_string(&path)?;
        let (frontmatter, _) = original
            .split_once("\n---\n")
            .context("fixture frontmatter")?;
        std::fs::write(
            path,
            format!(
                "{frontmatter}\n---\n\n{}\n\n[Reference link](https://example.invalid/reference)\n\n```text\n{}\n```\n",
                format!("{body}\n\n").repeat(15),
                "A deliberately wide line for horizontal code scrolling. ".repeat(16)
            ),
        )?;
    }
    crate::harness::git(&archive, &["add", "items"])?;
    crate::harness::git(
        &archive,
        &["commit", "-qm", "distinct gesture fixture articles"],
    )?;
    fixture.build()?;
    let client = browser_client().await?;
    let result=async {
        phone_session(&client).await?;
        let start=format!("{}items/example/2026-09-01-story-29/",fixture.base);
        client.goto(&start).await?;
        wait_booted_with(&client,"document.querySelector('article.item')?.dataset.nextUrl && document.querySelector('article.item')?.dataset.previousUrl").await?;
        // Reading gestures belong to the page: nothing here steals them to turn the article.
        client.execute("scrollTo(0,100)",vec![]).await?;
        swipe(&client,(190.0,620.0),(180.0,300.0)).await?;
        wait_for(&client,"scrollY>200").await?;
        anyhow::ensure!(client.current_url().await?.as_str()==start,"vertical reading gestures must not change article");
        // A wide code block scrolls sideways on its own, under the reader's finger, and the
        // article underneath stays where it is.
        client.execute("document.querySelector('.body pre').scrollIntoView({block:'center'})",vec![]).await?;
        let code=client.execute("const pre=document.querySelector('.body pre');return {overflow:getComputedStyle(pre).overflowX,wider:pre.scrollWidth>pre.clientWidth+1}",vec![]).await?;
        anyhow::ensure!(code["overflow"]=="auto" && code["wider"]==true,"wide code keeps its own horizontal scroll: {code}");
        client.execute("const pre=document.querySelector('.body pre');pre.scrollLeft=80",vec![]).await?;
        wait_for(&client,"document.querySelector('.body pre').scrollLeft>0").await?;
        anyhow::ensure!(client.current_url().await?.as_str()==start,"horizontal code scrolling must not turn the article");
        // The keys that do turn it take the whole page with them, in both directions.
        let neighbors=client.execute("const article=document.querySelector('article.item');return {next:new URL(article.dataset.nextUrl,new URL(window.AGGR.base,location.href)).href,previous:new URL(article.dataset.previousUrl,new URL(window.AGGR.base,location.href)).href}",vec![]).await?;
        client.execute("document.activeElement?.blur()",vec![]).await?;
        key(&client,"j").await?;
        wait_booted_with(&client,&format!("location.href==={}",neighbors["next"])).await?;
        anyhow::ensure!(client.execute("return scrollY",vec![]).await?==0,"a turned page starts at the top of its own article");
        key(&client,"k").await?;
        wait_booted_with(&client,&format!("location.href==={}",json!(start))).await?;
        Ok(())
    }.await;
    report_failure(&client, "mobile-article-gestures", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn mobile_feed_search_stays_below_header_while_articles_and_results_scroll() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let config = fixture.directory.path().join("aggr.toml");
    std::fs::write(
        &config,
        std::fs::read_to_string(&config)?.replace("items_per_page=3", "items_per_page=30"),
    )?;
    fixture.build()?;
    let client = browser_client().await?;
    let result=async {
        phone_session(&client).await?;
        for width in [390,1024] {
            emulate(&client,"Emulation.setDeviceMetricsOverride",json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":width<640})).await?;
            client.goto(&fixture.base).await?;
            wait_booted_with(&client,"document.querySelectorAll('[data-static-feed] .row').length>=20").await?;
            client.execute("scrollTo(0,700)",vec![]).await?;
            wait_for(&client,"scrollY>=600 && Math.abs(document.querySelector('.feed-toolbar').getBoundingClientRect().top-document.querySelector('.top').getBoundingClientRect().bottom)<1").await?;
            let pinned=client.execute(r#"
              const toolbar=document.querySelector('.feed-toolbar'),input=document.querySelector('#q'),box=input.getBoundingClientRect();
              return {position:getComputedStyle(toolbar).position,visible:document.elementFromPoint(box.x+box.width/2,box.y+box.height/2)===input,
                separateResults:!toolbar.querySelector('[data-search-results]'),tabs:[...document.querySelectorAll('.mobile-tabs a')].map(a=>a.textContent.trim()),scroll:scrollY};
            "#,vec![]).await?;
            anyhow::ensure!(pinned["position"]=="sticky" && pinned["visible"]==true && pinned["separateResults"]==true && pinned["tabs"]==json!(["feed","browse","preferences"]),"only the search field stays pinned below the header: {pinned}");
            key(&client,"/").await?;
            wait_for(&client,"document.activeElement?.id==='q'").await?;
            let focused_scroll=client.execute("return scrollY",vec![]).await?;
            anyhow::ensure!((focused_scroll.as_f64().unwrap()-pinned["scroll"].as_f64().unwrap()).abs()<1.0,"focusing an already visible pinned search keeps the reading position");
            client.find(Locator::Css("#q")).await?.send_keys("source:\"publisher.invalid\"").await?;
            wait_for(&client,"document.querySelectorAll('[data-search-results] .row').length>=20").await?;
            client.execute("document.querySelector('#q').blur();scrollTo(0,700)",vec![]).await?;
            wait_for(&client,"scrollY>=600 && Math.abs(document.querySelector('.feed-toolbar').getBoundingClientRect().top-document.querySelector('.top').getBoundingClientRect().bottom)<1").await?;
            let results=client.execute("return {query:document.querySelector('#q').value,active:document.querySelector('.mobile-tabs [aria-current]')?.dataset.route,resultsTop:document.querySelector('[data-search-results]').getBoundingClientRect().top,fieldBottom:document.querySelector('.feed-toolbar').getBoundingClientRect().bottom}",vec![]).await?;
            anyhow::ensure!(results["query"]=="source:\"publisher.invalid\"" && results["active"]=="" && results["resultsTop"].as_f64().unwrap()<results["fieldBottom"].as_f64().unwrap(),"results scroll independently under the field while Feed stays active: {results}");
            if width<640 {
                let keyboard=client.execute_async(r#"
                  const done=arguments[arguments.length-1],input=document.querySelector('#q'),viewport=visualViewport,bar=document.querySelector('.mobile-tabs'),before=scrollY;
                  input.focus({preventScroll:true});Object.defineProperty(viewport,'height',{configurable:true,value:innerHeight-300});viewport.dispatchEvent(new Event('resize'));
                  const result={hidden:getComputedStyle(bar).visibility==='hidden',searchVisible:input.getBoundingClientRect().top>=document.querySelector('.top').getBoundingClientRect().bottom && input.getBoundingClientRect().bottom<viewport.height,stable:scrollY===before};
                  input.blur();delete viewport.height;viewport.dispatchEvent(new Event('resize'));queueMicrotask(()=>done(result));
                "#,vec![]).await?;
                anyhow::ensure!(keyboard==json!({"hidden":true,"searchVisible":true,"stable":true}),"keyboard leaves the pinned search visible without moving content: {keyboard}");
                screenshot(&client,"mobile-pinned-feed-search").await?;
            }
            client.find(Locator::Css("#q")).await?.clear().await?;
            client.find(Locator::Css("#q")).await?.send_keys("source:").await?;
            wait_for(&client,"document.querySelector('.search-completions')?.getBoundingClientRect().height>0").await?;
            anyhow::ensure!(client.execute("const input=document.querySelector('#q').getBoundingClientRect(),list=document.querySelector('.search-completions').getBoundingClientRect();return list.top>=input.bottom && list.bottom<=innerHeight",vec![]).await?==true,"completion remains visible below the pinned input");
        }
        Ok(())
    }.await;
    report_failure(&client, "mobile-pinned-search", &result).await;
    finish(client, result).await
}
