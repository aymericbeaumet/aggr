//! Search: completion, results, keyboard selection and the query language.

use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context as _, Result};
use fantoccini::{Client, Locator};
use serde_json::{Value, json};

use crate::harness::{
    Fixture, browser_client, catch_panics, emulate, escape_search, finish, fixture_pdf, git, key,
    phone_session, report_failure, screenshot, set_offline, wait_booted, wait_booted_with,
    wait_for,
};

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn search_results_keyboard_selection_and_history() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::new()?;
    let client = browser_client().await?;
    let result = catch_panics(search_keyboard_contracts(&client, &fixture)).await;
    report_failure(&client, "search-keyboard", &result).await;
    finish(client, result).await
}

async fn search_keyboard_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    phone_session(client).await?;
    client.goto(&fixture.base).await?;
    wait_booted(client).await?;
    client
        .execute(
            "localStorage.setItem('aggr:single-key-shortcuts','true');localStorage.setItem('aggr:feed-page-size','25')",
            vec![],
        )
        .await?;
    client.goto(&format!("{}?q=article", fixture.base)).await?;
    wait_for(
        client,
        "document.querySelectorAll('#list .row').length === 25",
    )
    .await?;
    assert_eq!(
        client
            .execute("return new URL(location.href).href===location.href", vec![])
            .await?,
        true,
        "a canonical search keeps its own address"
    );
    let input = client.find(Locator::Css("#q")).await?;
    input.clear().await?;
    input.send_keys("j").await?;
    assert_eq!(input.prop("value").await?, Some("j".into()));
    input.clear().await?;
    input.send_keys("article").await?;
    wait_for(
        client,
        "document.querySelector('#q').value === 'article' && document.querySelector('#list')?.getAttribute('aria-busy') === 'false' && document.querySelectorAll('#list .row').length === 25",
    )
    .await?;
    assert_eq!(client.execute("return document.querySelector('#list .row.is-selected') === document.querySelector('#list .row') && document.activeElement.id === 'q'", vec![]).await?, true,
        "search results select the first item while leaving typing focus in search");
    assert_eq!(client.execute(r#"
      const rows = [...document.querySelectorAll('#list .row')];
      const edge = rows[0].getBoundingClientRect().left + parseFloat(getComputedStyle(rows[0], '::after').left);
      return rows.every(row => {
        const separator = row.getBoundingClientRect().left + parseFloat(getComputedStyle(row, '::after').left);
        const marker = getComputedStyle(row, '::before');
        const markerRight = row.getBoundingClientRect().left + parseFloat(marker.left) + parseFloat(marker.width);
        return Math.abs(edge - separator) < 1 && Math.abs(markerRight - separator) < 1;
      });
    "#, vec![]).await?, true, "all separators must share one left edge and meet the selection bar");
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
    // The search comes back with its cursor where it was left, without taking the keyboard.
    wait_for(client, "document.querySelectorAll('#list .row').length === 25 && !!document.querySelector('#list .row.is-selected')").await?;
    assert_eq!(
        client
            .execute(
                "return document.querySelector('#list .row.is-selected [data-row-open]').href",
                vec![]
            )
            .await?,
        selected_search
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn contextual_source_completion_and_item_types() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let root = fixture.directory.path();
    let config = root.join("aggr.toml");
    std::fs::write(
        &config,
        format!(
            "{}\n[[sources]]\nurl='https://open.spotify.com/show/opaque123'\nslug='open-spotify-com-show-opaque123'\nname='Underscore_'\ncategory='News'\n",
            std::fs::read_to_string(&config)?
        ),
    )?;
    let archive = root.join(".aggr/data");
    let directory = archive.join("items/open-spotify-com-show-opaque123/2026/09");
    std::fs::create_dir_all(&directory)?;
    std::fs::write(
        directory.join("2026-09-01-podcast.md"),
        "---\ntitle: A podcast episode\nlink: https://open.spotify.com/episode/episode123\nsource: open-spotify-com-show-opaque123\npublished: 2026-09-01T12:00:00Z\nfirst_seen: 2026-09-01T12:00:00Z\ncontent: feed\n---\n\nA discussion about technology.\n",
    )?;
    git(&archive, &["add", "items"])?;
    git(&archive, &["commit", "-qm", "fixture podcast source"])?;
    fixture.build()?;
    let client = browser_client().await?;
    let result = async {
        client.goto(&fixture.base).await?;
        wait_for(&client, "!!document.querySelector('.search-input-line')").await?;
        let local_publisher = url::Url::parse(&fixture.base)?;
        let local_publisher = local_publisher.host_str().unwrap().to_string();
        for (query, label, expected) in [
            ("category:engineering source:", "Example", json!({
                "Example": "source · publisher.invalid · 45", local_publisher: "source · 1",
                "twitch.tv/videos": "source · 1", "vimeo.com/123456789": "source · 1",
                "youtube.com": "source · 1",
            })),
            ("category:news source:", "Underscore_", json!({
                "Underscore_": "source · open.spotify.com/show/opaque123 · 1",
            })),
            ("type:podcast source:", "Underscore_", json!({
                "Underscore_": "source · open.spotify.com/show/opaque123 · 1",
            })),
        ] {
            client.execute("const q=document.querySelector('#q');q.focus();q.value=arguments[0];q.setSelectionRange(q.value.length,q.value.length);q.dispatchEvent(new Event('input',{bubbles:true}))",vec![json!(query)]).await?;
            wait_for(&client,&format!("[...document.querySelectorAll('.completion-label')].some(label=>label.textContent==={})",json!(label))).await?;
            // The counts start as the whole archive's and are refined against the rest of the
            // query; the refined list is the one the reader is offered.
            wait_for(&client,&format!("document.querySelectorAll('.search-completion').length==={}",expected.as_object().map(|map| map.len()).unwrap_or(0))).await?;
            let completions = client.execute("return Object.fromEntries([...document.querySelectorAll('.search-completion')].map(entry=>[entry.querySelector('.completion-label').textContent,entry.querySelector('small').textContent]))",vec![]).await?;
            anyhow::ensure!(completions == expected, "{query}: publisher and feed counts reflect the other active clauses: {completions}, expected {expected}");
        }
        client.execute("const q=document.querySelector('#q');q.focus();q.value='type:podcast source:under';q.setSelectionRange(q.value.length,q.value.length);q.dispatchEvent(new Event('input',{bubbles:true}))",vec![]).await?;
        wait_for(&client,"document.querySelectorAll('.search-completion').length===1 && document.querySelector('.completion-label')?.textContent==='Underscore_'").await?;
        key(&client,"Enter").await?;
        wait_for(&client,"document.querySelector('#q').value.includes('source:open.spotify.com/show/opaque123') && !document.querySelector('.search-completions:not([hidden])') && document.querySelector('#search-status')?.textContent==='1 article'").await?;
        anyhow::ensure!(client.execute("return document.querySelector('.search-results .title').textContent",vec![]).await?=="A podcast episode","Enter selects the named show");
        // A show is the publisher of its episodes: the catalogue path names it, and the episode's
        // own path names nothing anyone follows.
        let publisher=client.execute("return document.querySelector('.search-results .source-resolved').textContent",vec![]).await?;
        anyhow::ensure!(publisher=="open.spotify.com/show/opaque123","podcast publisher metadata names the show: {publisher}");
        anyhow::ensure!(client.execute("return document.querySelectorAll('.search-results .source-feed').length",vec![]).await?==0,"same-host podcast publisher and feed share one source membership");
        search_query(&client,"source:youtube.com",1).await?;
        let publisher_hit = client.execute("return document.querySelector('.search-results .title').href",vec![]).await?;
        let sources=client.execute("return [...document.querySelectorAll('.search-results .domain a')].map(link=>link.textContent)",vec![]).await?;
        anyhow::ensure!(sources==json!(["youtube.com", "publisher.invalid"]),"publisher and originating feed both have source links: {sources}");
        search_query(&client,"source:publisher.invalid source:youtube.com",45).await?;
        search_query(&client,"source:publisher.invalid type:video",3).await?;
        anyhow::ensure!(client.execute("return [...document.querySelectorAll('.search-results .title')].filter(link=>link.href===arguments[0]).length",vec![publisher_hit]).await?==1,"feed search contains the same canonical publisher article once");
        search_query(&client,"type:document",1).await?;
        anyhow::ensure!(client.execute("return document.querySelector('.search-results .u-bookmark-of').href.endsWith('document.pdf')",vec![]).await?==true,"PDFs are searchable as documents");
        Ok(())
    }.await;
    report_failure(&client, "contextual-completion", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn search_articles_only_appear_as_results() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        client.goto(&fixture.base).await?;
        wait_for(&client,"!!document.querySelector('.search-command')").await?;
        let nav=client.execute("const nav=document.querySelector('.nav-primary');return {labels:[...nav.querySelectorAll('a')].map(a=>a.textContent.trim()),pipes:nav.querySelectorAll('.nav-separator').length,feed:nav.querySelector('a').href}",vec![]).await?;
        anyhow::ensure!(nav["labels"]==json!(["feed","browse","preferences"]) && nav["pipes"]==2 && nav["feed"]==fixture.base,"desktop menu exposes feed | browse | preferences: {nav}");
        client.find(Locator::Css("#q")).await?.send_keys("Article").await?;
        wait_for(&client,"document.querySelectorAll('.search-results .row').length>1").await?;
        anyhow::ensure!(client.execute("return document.querySelectorAll('.search-completion').length",vec![]).await?==0,"matching article titles must remain in results, never autocomplete");
        // Results are the feed filtered: the cursor starts on the first one, and the static feed
        // waiting hidden behind them never holds it.
        let start=client.execute("const shown=[...document.querySelectorAll('.search-results .row')],hidden=[...document.querySelectorAll('[data-static-feed] .row')];return {selected:shown.findIndex(row=>row.classList.contains('is-selected')),stray:hidden.some(row=>row.classList.contains('is-selected'))}",vec![]).await?;
        anyhow::ensure!(start["selected"]==0 && start["stray"]==false,"the first result is selected by default: {start}");
        // Without suggestions the arrows move the result cursor while typing continues in the field.
        key(&client,"ArrowDown").await?;
        let cursor=client.execute("const rows=[...document.querySelectorAll('.search-results .row')];return {selected:rows.findIndex(row=>row.classList.contains('is-selected')),focused:document.activeElement.id,url:rows[1]?.querySelector('[data-row-open]')?.href}",vec![]).await?;
        anyhow::ensure!(cursor["selected"]==1 && cursor["focused"]=="q","ArrowDown selects the second result without leaving the search field: {cursor}");
        key(&client,"ArrowUp").await?;
        anyhow::ensure!(client.execute("return [...document.querySelectorAll('.search-results .row')].findIndex(row=>row.classList.contains('is-selected'))",vec![]).await?==0,"ArrowUp moves the cursor back to the first result");
        key(&client,"ArrowDown").await?;
        key(&client,"Enter").await?;
        wait_for(&client,"document.body.dataset.kind==='item'").await?;
        anyhow::ensure!(client.execute("return location.href",vec![]).await?==cursor["url"],"Enter opens the selected result when no suggestion is offered");
        client.back().await?;
        // Coming back shows the page that was left before the index has finished reloading.
        wait_for(&client,"document.body.dataset.kind==='river' && document.querySelectorAll('.search-results .row').length>1 && !document.querySelector('[data-static-feed]:not([hidden])')").await?;
        anyhow::ensure!(client.execute("return document.querySelector('#search-status')?.textContent!=='Searching…'",vec![]).await?==true,"returning to a search never shows an empty placeholder over kept results");
        wait_for(&client,"!!document.querySelector('#q') && new URL(location.href).searchParams.get('q')==='Article'").await?;
        // Out of the field, the results answer the ordinary feed shortcuts on the same cursor.
        escape_search(&client).await?;
        const CURSOR: &str = "return [...document.querySelectorAll('.search-results .row')].findIndex(row=>row.classList.contains('is-selected'))";
        let resumed=client.execute(CURSOR,vec![]).await?.as_i64().unwrap_or(-1);
        key(&client,"j").await?;
        let stepped=client.execute("const rows=[...document.querySelectorAll('.search-results .row')];return {selected:rows.findIndex(row=>row.classList.contains('is-selected')),focused:document.activeElement.id}",vec![]).await?;
        anyhow::ensure!(stepped["selected"].as_i64().unwrap_or(-1)==resumed+1 && stepped["focused"]!="q","j walks the search results once the field is left: {stepped}");
        key(&client,"k").await?;
        anyhow::ensure!(client.execute(CURSOR,vec![]).await?==resumed,"k walks back up the search results");
        // A half-typed qualifier is completed in two steps: the name, then the values it accepts.
        client.execute("const q=document.querySelector('#q');q.focus();q.value='Article sour';q.setSelectionRange(12,12);q.dispatchEvent(new Event('input',{bubbles:true}))",vec![]).await?;
        wait_for(&client,"document.querySelector('.search-completion')?.dataset.completionId==='source:'").await?;
        key(&client,"Enter").await?;
        wait_for(&client,"document.querySelector('#q').value==='Article source:' && [...document.querySelectorAll('.search-completion')].some(row=>row.dataset.completionId.startsWith('source:'))").await?;
        client.execute("const q=document.querySelector('#q');q.focus();q.value='source:';q.dispatchEvent(new Event('input',{bubbles:true}))",vec![]).await?;
        wait_for(&client,"document.querySelectorAll('.search-completion').length>1 && document.querySelector('.search-completion')?.dataset.completionId.startsWith('source:')").await?;
        // Open suggestions take the arrows; the results keep them the rest of the time.
        key(&client,"ArrowDown").await?;
        let moved=client.execute("const rows=[...document.querySelectorAll('.search-completion')];return {highlighted:rows.findIndex(row=>row.getAttribute('aria-selected')==='true'),focused:document.activeElement.id}",vec![]).await?;
        anyhow::ensure!(moved["highlighted"]==1 && moved["focused"]=="q","the arrows walk open suggestions: {moved}");
        client.find(Locator::Css(".nav-primary [data-feed-action]")).await?.click().await?;
        wait_for(&client,"!new URL(location.href).searchParams.has('q') && !document.querySelector('[data-static-feed]').hidden").await?;
        client.find(Locator::Css(".brand")).await?.click().await?;
        anyhow::ensure!(client.execute("return document.activeElement.id!=='q' && scrollY===0 && !document.querySelector('.search-completions:not([hidden])')",vec![]).await?==true,"site title returns to the feed top without focusing search");
        Ok(())
    }.await;
    report_failure(&client, "search-articles-only", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn search_preview_errors_keep_geometry_and_readable_fallbacks() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        client.goto(&format!("{}?q=source:publisher.invalid", fixture.base)).await?;
        wait_for(&client, "!!document.querySelector('.search-results .preview-image')?.naturalWidth").await?;
        let before = client.execute("const image=document.querySelector('.search-results .preview-image');window.previewImage=image;window.previewSource=image.src;const r=image.parentElement.getBoundingClientRect();image.src=new URL('missing-preview.png',new URL(window.AGGR.base,location.href)).href;return {width:r.width,height:r.height}", vec![]).await?;
        wait_for(&client,"window.previewImage.parentElement.classList.contains('is-error')").await?;
        let failed=client.execute("const image=window.previewImage,r=image.parentElement.getBoundingClientRect();return {width:r.width,height:r.height,color:getComputedStyle(image).color,alt:image.alt}",vec![]).await?;
        anyhow::ensure!(before["width"]==failed["width"] && before["height"]==failed["height"] && failed["color"]!="rgba(0, 0, 0, 0)" && failed["alt"].as_str().is_some_and(|alt|!alt.is_empty()),"search image failures preserve space and readable alt text: {failed}");
        client.execute("window.previewImage.src=window.previewSource",vec![]).await?;
        wait_for(&client,"window.previewImage.naturalWidth>0 && window.previewImage.parentElement.classList.contains('is-loaded') && !window.previewImage.parentElement.classList.contains('is-error')").await?;
        Ok(())
    }.await;
    report_failure(&client, "search-preview-errors", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn search_control_mount_and_delayed_facets() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let metrics = r#"
      const input=document.querySelector('#q'),r=input.getBoundingClientRect(),s=getComputedStyle(input);
      return {placeholder:input.placeholder,placeholderColor:getComputedStyle(input,'::placeholder').color,placeholderOpacity:getComputedStyle(input,'::placeholder').opacity,width:r.width,height:r.height,top:r.top,left:r.left,background:s.backgroundColor,padding:s.padding,border:s.borderRadius,font:s.font,feedTop:document.querySelector('[data-static-feed]').getBoundingClientRect().top,icons:document.querySelectorAll('[data-search-root] svg').length};
    "#;
    let styles_loaded = "document.querySelectorAll('link[rel=stylesheet]').length > 0 && [...document.querySelectorAll('link[rel=stylesheet]')].every(link => link.sheet)";
    let result = async {
        for width in [1280,390] {
            emulate(&client,"Emulation.setDeviceMetricsOverride",json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":width<600})).await?;
            fixture.scripts_blocked.store(true,Ordering::Relaxed);
            client.goto(&fixture.base).await?;
            wait_for(&client, styles_loaded).await?;
            anyhow::ensure!(client.execute("return document.documentElement.dataset.aggrReady !== 'true' && !!document.querySelector('.search-command')",vec![]).await?==true,"the field is part of the generated page, sampled with scripts still blocked");
            let before=client.execute(metrics,vec![]).await?;
            fixture.scripts_blocked.store(false,Ordering::Relaxed);
            client.goto(&fixture.base).await?;
            wait_for(&client,"document.querySelector('.search-input-line')").await?;
            wait_for(&client, styles_loaded).await?;
            let after=client.execute(metrics,vec![]).await?;
            anyhow::ensure!(before==after,"mounting search must preserve its grey field and surrounding geometry at {width}px: before={before}, after={after}");
            anyhow::ensure!(after["icons"]==0 && after["background"]!="rgba(0, 0, 0, 0)","search needs a filled input without a magnifier: {after}");
            client.find(Locator::Css("#q")).await?.send_keys("reading").await?;
            wait_for(&client,"document.querySelector('.search-clear')").await?;
            anyhow::ensure!(client.execute("return getComputedStyle(document.querySelector('#q')).boxShadow",vec![]).await?=="none","search focus must not have a heavy bottom shadow");
            screenshot(&client,&format!("search-clear-focused-{width}")).await?;
            let clear=client.execute(r#"
              const q=document.querySelector('#q').getBoundingClientRect(),button=document.querySelector('.search-clear'),r=button.getBoundingClientRect();
              return {inside:r.left>=q.left&&r.right<=q.right+1&&r.top>=q.top&&r.bottom<=q.bottom+1,large:r.width>=44&&r.height>=44,text:button.textContent.trim(),label:button.getAttribute('aria-label')};
            "#,vec![]).await?;
            anyhow::ensure!(clear["inside"]==true && clear["large"]==true && clear["text"]=="×" && clear["label"]=="Clear search","clear must be an accessible inline cross with a touch target: {clear}");
        }
        emulate(&client,"Page.addScriptToEvaluateOnNewDocument",json!({"source":r#"
          const originalFetch=window.fetch;
          const waiting=[];
          window.fetch=async function(input,...rest){
            if(String(input?.url||input).includes('search-catalog.json')){
              await new Promise(resolve=>{
                waiting.push(resolve);
                window.releaseSearchManifest=()=>waiting.splice(0).forEach(release=>release());
              });
            }
            return originalFetch.call(this,input,...rest);
          };
        "#})).await?;
        client.goto(&fixture.base).await?;
        wait_for(&client,"document.querySelector('.search-input-line')").await?;
        client.find(Locator::Css("#q")).await?.send_keys("source:").await?;
        wait_for(&client,"typeof window.releaseSearchManifest==='function'").await?;
        // The page belongs to the query from the moment it is asked, even though the catalogue
        // that answers it is still on its way.
        wait_for(&client,"document.body.hasAttribute('data-searching')").await?;
        anyhow::ensure!(client.execute("return document.querySelector('[data-static-feed]').hidden && !document.querySelector('.search-results .row')",vec![]).await?==true,"feed rows stay hidden while the search catalogue loads");
        client.execute("window.releaseSearchManifest()",vec![]).await?;
        wait_for(&client,"document.querySelectorAll('.search-completion').length>0").await?;
        let offered=client.execute("return [...document.querySelectorAll('.search-completion')].map(row=>row.textContent)",vec![]).await?;
        anyhow::ensure!(offered.as_array().is_some_and(|rows|rows.iter().any(|row|row.as_str().unwrap_or_default().to_lowercase().contains("example"))),"the released catalogue names the feeds it knows: {offered}");
        key(&client,"\u{e015}").await?;
        key(&client,"\u{e013}").await?;
        key(&client,"Tab").await?;
        wait_for(&client,"document.querySelector('#q').value.trim()==='source:publisher.invalid' && !document.querySelector('.search-completions:not([hidden])')").await?;
        wait_for(&client,"document.querySelector('.search-results .row')").await?;
        Ok(())
    }.await;
    fixture.scripts_blocked.store(false, Ordering::Relaxed);
    report_failure(&client, "search-control-mount", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn search_focus_keeps_the_reading_position() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let config = fixture.directory.path().join("aggr.toml");
    std::fs::write(
        &config,
        std::fs::read_to_string(&config)?.replace("items_per_page=3", "items_per_page=45"),
    )?;
    fixture.build()?;
    let client = browser_client().await?;
    let result = async {
        for width in [1280, 390] {
            emulate(&client, "Emulation.setDeviceMetricsOverride", json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":width<600})).await?;
            client.goto(&fixture.base).await?;
            wait_booted_with(&client, "!!document.querySelector('.search-command')").await?;
            let visible_y = client.execute("const q=document.querySelector('#q').getBoundingClientRect(), header=document.querySelector('.top').getBoundingClientRect();return Math.max(1,Math.floor((q.top-header.bottom)/2))", vec![]).await?;
            for modifier in ["metaKey", "ctrlKey"] {
                client.execute("document.querySelector('#q').blur();window.scrollTo({top:arguments[0],behavior:'instant'})", vec![visible_y.clone()]).await?;
                client.find(Locator::Css("#q")).await?.click().await?;
                anyhow::ensure!(client.execute("return Math.abs(scrollY-arguments[0])<=1 && document.activeElement.id==='q'",vec![visible_y.clone()]).await?==true,"clicking a fully visible search input preserves scroll at {width}px");
                client.execute(&format!("document.dispatchEvent(new KeyboardEvent('keydown',{{key:'k',{modifier}:true,bubbles:true,cancelable:true}}))"),vec![]).await?;
                anyhow::ensure!(client.execute("return Math.abs(scrollY-arguments[0])<=1",vec![visible_y.clone()]).await?==true,"{modifier} preserves scroll when already focused and visible at {width}px");
                // The field is pinned under the header, so it is never out of reach: reaching for
                // it from deep in the feed must not cost the reader their place.
                client.execute("document.querySelector('#q').blur();window.scrollTo({top:650,behavior:'instant'})",vec![]).await?;
                wait_for(&client,"scrollY>500").await?;
                client.execute(&format!("document.dispatchEvent(new KeyboardEvent('keydown',{{key:'k',{modifier}:true,bubbles:true,cancelable:true}}))"),vec![]).await?;
                wait_for(&client,"document.activeElement.id==='q'").await?;
                anyhow::ensure!(client.execute("return Math.abs(scrollY-650)<=1",vec![]).await?==true,"{modifier} reaches the pinned field without scrolling at {width}px");
            }
            client.execute("document.querySelector('#q').blur();const q=document.querySelector('#q').getBoundingClientRect(),header=document.querySelector('.top').getBoundingClientRect();window.scrollTo({top:q.top-header.bottom+q.height/2,behavior:'instant'})",vec![]).await?;
            let point=client.execute("const q=document.querySelector('#q').getBoundingClientRect();return {x:q.left+20,y:q.bottom-4}",vec![]).await?;
            emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mousePressed","x":point["x"],"y":point["y"],"button":"left","clickCount":1})).await?;
            emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mouseReleased","x":point["x"],"y":point["y"],"button":"left","clickCount":1})).await?;
            wait_for(&client,"document.activeElement.id==='q'").await?;
            escape_search(&client).await?;
            anyhow::ensure!(client.execute("return document.activeElement.id!=='q' && !document.querySelector('.search-completions:not([hidden])')",vec![]).await?==true,"Escape closes suggestions, then blurs search");
        }
        Ok(())
    }.await;
    if result.is_err() {
        eprintln!("search focus state: {}", client.execute("return {scroll:scrollY,height:document.documentElement.scrollHeight,input:document.querySelector('#q').getBoundingClientRect().toJSON(),header:document.querySelector('.top').getBoundingClientRect().toJSON(),active:document.activeElement.id}",vec![]).await.unwrap_or(Value::Null));
    }
    report_failure(&client, "search-focus-visibility", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn rich_search_across_the_query_language() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::new()?;
    let archive = fixture.directory.path().join(".aggr/data");
    for (index, text) in [
        (1, "Cobalt marmalade."),
        (44, "Rare exclusion."),
        (45, "Rare exclusion."),
    ] {
        let path = archive.join(format!(
            "items/example/2026/09/2026-09-01-story-{index:02}.md"
        ));
        let body = std::fs::read_to_string(&path)?;
        std::fs::write(path, format!("{body}\n\n{text}\n"))?;
    }
    git(&archive, &["add", "items"])?;
    git(&archive, &["commit", "-qm", "fixture search vocabulary"])?;
    let body_image = std::fs::read(fixture.out.join("body.png"))?;
    fixture.build()?;
    std::fs::write(fixture.out.join("body.png"), body_image)?;
    std::fs::write(fixture.out.join("document.pdf"), fixture_pdf())?;
    let client = browser_client().await?;
    let result = rich_search_contracts(&client, &fixture).await;
    report_failure(&client, "rich-search", &result).await;
    if result.is_err() {
        eprintln!("search state: {}", client.execute("return {error:document.querySelector('.search-error')?.textContent,status:document.querySelector('#search-status')?.textContent,rows:document.querySelectorAll('.search-results .row').length}", vec![]).await.unwrap_or(Value::Null));
        let published: Value = std::fs::read(fixture.out.join("search-manifest.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or(Value::Null);
        eprintln!(
            "published search manifest: {}",
            json!({"version":published["version"],"docs":published["docs"]})
        );
        eprintln!("browser search manifest: {}", client.execute_async("const done=arguments[arguments.length-1];fetch(new URL('search-manifest.json',new URL(window.AGGR.base,location.href)),{cache:'no-store'}).then(r=>r.json()).then(m=>done({version:m.version,docs:m.docs})).catch(e=>done(String(e)))",vec![]).await.unwrap_or(Value::Null));
    }
    let _ = set_offline(&client, false).await;
    finish(client, result).await
}

async fn search_query(client: &Client, query: &str, count: usize) -> Result<()> {
    // Two queries in a row can want the same number of articles, so the count only answers for
    // this one once the previous answer has been cleared away.
    client
        .execute(
            "const status=document.querySelector('#search-status');if(status)status.textContent=''",
            vec![],
        )
        .await?;
    client.execute("const input=document.querySelector('#q');input.focus();input.value=arguments[0];input.setSelectionRange(input.value.length,input.value.length);input.dispatchEvent(new Event('input',{bubbles:true}));document.querySelector('#search-form').dispatchEvent(new Event('submit',{bubbles:true,cancelable:true}))", vec![json!(query)]).await?;
    wait_for(client, &format!("document.querySelector('#search-status')?.textContent === '{} {}' && !document.querySelector('.search-error:not([hidden])')", count, if count == 1 { "article" } else { "articles" })).await?;
    anyhow::ensure!(
        client
            .execute("return document.activeElement?.id", vec![])
            .await?
            == "q",
        "search results must not steal the typing focus"
    );
    Ok(())
}

async fn rich_search_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    client.goto(&fixture.base).await?;
    client
        .execute("localStorage.setItem('aggr:feed-page-size','25')", vec![])
        .await?;
    client.refresh().await?;
    wait_for(
        client,
        "navigator.serviceWorker.controller && document.querySelector('#q')",
    )
    .await?;
    anyhow::ensure!(client.execute("return getComputedStyle(document.querySelector('#search-query-help')).display==='none' && document.querySelector('#q').getAttribute('aria-expanded')==='false'", vec![]).await? == true, "query help and completion must start collapsed");
    let before_help = client
        .execute(
            "return document.querySelector('[data-static-feed]').getBoundingClientRect().top",
            vec![],
        )
        .await?;
    client
        .execute("document.querySelector('#q').focus()", vec![])
        .await?;
    anyhow::ensure!(
        client.execute("return getComputedStyle(document.querySelector('#search-query-help')).display==='none'", vec![]).await? == true,
        "keyboard focus alone must not open the help"
    );
    client.execute("document.querySelector('.search-control').dispatchEvent(new PointerEvent('pointerenter',{pointerType:'touch'}))",vec![]).await?;
    anyhow::ensure!(
        client.execute("return getComputedStyle(document.querySelector('#search-query-help')).display==='none'", vec![]).await? == true,
        "touch must not open the help"
    );
    // The pointer can only rest on the field if the field is on screen: a small window puts it
    // below the fold, and a move to a point outside the viewport hovers nothing.
    let hover=client.execute("const q=document.querySelector('#q');q.scrollIntoView({block:'center'});const r=q.getBoundingClientRect();return {x:Math.round(r.left+20),y:Math.round(r.top+r.height/2),inside:r.top>=0&&r.bottom<=innerHeight&&r.left>=0&&r.right<=innerWidth}",vec![]).await?;
    anyhow::ensure!(
        hover["inside"] == true,
        "the search field must be on screen before the pointer can rest on it: {hover}"
    );
    emulate(
        client,
        "Input.dispatchMouseEvent",
        json!({"type":"mouseMoved","x":hover["x"],"y":hover["y"]}),
    )
    .await?;
    let pointer = client
        .execute(
            "const field=document.querySelector('#q'),help=document.querySelector('#search-query-help');return {hovered:field.matches(':hover'),focused:field.matches(':focus'),fine:matchMedia('(hover: hover) and (pointer: fine)').matches,display:getComputedStyle(help).display,role:help.getAttribute('role')}",
            vec![],
        )
        .await?;
    anyhow::ensure!(
        pointer["hovered"] == true && pointer["focused"] == true && pointer["role"] == "tooltip",
        "the reminder waits for both halves of the question: {pointer}"
    );
    // Only a browser with a pointer to rest has this question to ask. A headless one on a machine
    // with no pointing device does not, and must leave the reminder alone.
    anyhow::ensure!(
        (pointer["display"] != "none") == (pointer["fine"] == true),
        "a pointer resting on a focused field asks for the reminder; nothing else does: {pointer}"
    );
    // Hovering on the way past is not a question: without the field's attention the reminder stays
    // out of the way, and comes back when the pointer returns to a focused field.
    client
        .execute("document.querySelector('#q').blur()", vec![])
        .await?;
    wait_for(
        client,
        "getComputedStyle(document.querySelector('#search-query-help')).display==='none'",
    )
    .await?;
    client
        .execute("document.querySelector('#q').focus()", vec![])
        .await?;
    if pointer["fine"] == true {
        wait_for(
            client,
            "getComputedStyle(document.querySelector('#search-query-help')).display!=='none'",
        )
        .await?;
    }
    anyhow::ensure!(
        client
            .execute(
                "return document.querySelector('[data-static-feed]').getBoundingClientRect().top",
                vec![]
            )
            .await?
            == before_help,
        "query help overlays the page without shifting the feed"
    );
    // Leaving the field takes the reminder with it, along with anything the menu was offering.
    escape_search(client).await?;
    wait_for(client,"getComputedStyle(document.querySelector('#search-query-help')).display==='none' && document.querySelector('#q').getAttribute('aria-expanded')==='false'").await?;

    search_query(client, "\"cobalt marmalade\"", 1).await?;
    anyhow::ensure!(
        client
            .execute(
                "return document.querySelector('.search-results [data-row-open]').textContent",
                vec![]
            )
            .await?
            == "Article 1",
        "a phrase found only in one old article reaches it"
    );
    search_query(client, "\"rare exclusion\"", 2).await?;

    search_query(
        client,
        "source:publisher.invalid category:engineering tag:rust date:>=2026-09-02",
        22,
    )
    .await?;
    search_query(client, "comfortably \"current page\" -\"rare exclusion\" source:publisher.invalid category:engineering tag:reading", 43).await?;
    anyhow::ensure!(
        client
            .execute(
                "return document.querySelectorAll('.search-results .row').length",
                vec![]
            )
            .await?
            == 25,
        "the total must count all compound matches before pagination"
    );
    client
        .find(Locator::Css(
            "[data-search-results] [data-search-page-next]",
        ))
        .await?
        .click()
        .await?;
    // The stylesheet numbers the results, so the second page continues the count rather than
    // starting again at one.
    wait_for(
        client,
        "document.querySelectorAll('.search-results .row').length===18",
    )
    .await?;
    let numbering = client
        .execute(
            "return getComputedStyle(document.querySelector('.search-results')).counterReset",
            vec![],
        )
        .await?;
    anyhow::ensure!(
        numbering == "rank 25",
        "page two is numbered from 26: {numbering}"
    );
    anyhow::ensure!(
        client
            .execute(
                "return document.querySelector('#search-status').textContent",
                vec![]
            )
            .await?
            == "43 articles",
        "page changes must preserve the complete count"
    );

    client.execute("const q=document.querySelector('#q');q.focus();q.value='source:ex tag:rust';q.setSelectionRange(9,9);q.dispatchEvent(new Event('input',{bubbles:true}));q.dispatchEvent(new Event('select',{bubbles:true}))",vec![]).await?;
    wait_for(client,"document.querySelector('.search-completion[data-selected] .completion-label')?.textContent==='Example'").await?;
    let offered = client
        .execute(
            "return document.querySelector('.search-completion[data-selected]').dataset.completionId",
            vec![],
        )
        .await?;
    let state = client.execute("return {open:!document.querySelector('.search-completions').hidden,count:document.querySelectorAll('.search-completion').length,active:document.activeElement?.id||document.activeElement?.className,value:document.querySelector('#q').value,href:location.href,ready:document.readyState}",vec![]).await?;
    key(client, "Enter").await?;
    anyhow::ensure!(
        client
            .execute("return !!document.querySelector('#q')", vec![])
            .await?
            == true,
        "Enter must accept the completion rather than open a result: {state}"
    );
    // Only the half-typed clause is replaced, and a value with nothing to escape is written
    // plainly; the rest of the query is left exactly as it was.
    wait_for(client,&format!("document.querySelector('#q').value.includes({}) && document.querySelector('#q').value.includes('tag:rust')",json!(offered))).await?;
    anyhow::ensure!(
        client
            .execute("return document.activeElement?.id", vec![])
            .await?
            == "q",
        "completion retains editor focus"
    );
    wait_for(client,"!document.querySelector('.search-completions:not([hidden])') && document.querySelector('#q').getAttribute('aria-expanded')==='false'").await?;
    client.execute("const q=document.querySelector('#q');q.focus();q.value='sort:';q.setSelectionRange(5,5);q.dispatchEvent(new Event('input',{bubbles:true}))",vec![]).await?;
    wait_for(
        client,
        "document.querySelectorAll('.search-completion').length===3",
    )
    .await?;
    // Each arrow moves the highlight one step down the menu, and a re-render of the same choices
    // in between leaves it where the reader put it.
    const HIGHLIGHT: &str = "[...document.querySelectorAll('.search-completion')].findIndex(row=>row.hasAttribute('data-selected'))";
    wait_for(client, &format!("{HIGHLIGHT}===0")).await?;
    for step in [1, 2] {
        client
            .find(Locator::Css("#q"))
            .await?
            .send_keys("\u{e015}")
            .await?;
        wait_for(client, &format!("{HIGHLIGHT}==={step}")).await?;
    }
    let second_choice = client
        .execute(
            "return document.querySelector('.search-completion[data-selected] .completion-label')?.textContent",
            vec![],
        )
        .await?;
    anyhow::ensure!(
        second_choice.is_string(),
        "successive arrows must advance selection without a re-render resetting it"
    );
    let before_enter = client.execute("return {open:!document.querySelector('.search-completions').hidden,count:document.querySelectorAll('.search-completion').length,active:document.activeElement?.id,value:document.querySelector('#q').value}",vec![]).await?;
    key(client, "Enter").await?;
    anyhow::ensure!(
        client
            .execute("return !!document.querySelector('#q')", vec![])
            .await?
            == true,
        "Enter must accept the highlighted sort, not open a result: {before_enter}"
    );
    wait_for(client,&format!("document.querySelector('#q').value.trim()==={} && !document.querySelector('.search-completions:not([hidden])')",json!(format!("sort:{}",second_choice.as_str().unwrap_or_default())))).await?;
    wait_for(
        client,
        "document.querySelector('#search-status')?.textContent==='45 articles'",
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(250)).await;
    anyhow::ensure!(client.execute("return !document.querySelector('.search-completions:not([hidden])') && document.querySelector('#q').getAttribute('aria-expanded')==='false'",vec![]).await?==true,"selected completion must stay closed when results arrive");

    // Composition updates must not run or publish a half-composed query.
    client.execute("window.beforeCompositionURL=location.href;const q=document.querySelector('#q');q.dispatchEvent(new CompositionEvent('compositionstart',{bubbles:true}));q.value='cobalt';q.dispatchEvent(new InputEvent('input',{bubbles:true,isComposing:true}));q.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true,isComposing:true}))",vec![]).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    anyhow::ensure!(
        client
            .execute("return location.href===window.beforeCompositionURL", vec![])
            .await?
            == true,
        "IME composition must not submit partial text"
    );
    client.execute("document.querySelector('#q').dispatchEvent(new CompositionEvent('compositionend',{bubbles:true,data:'cobalt'}))",vec![]).await?;
    wait_for(
        client,
        "document.querySelector('#search-status')?.textContent==='1 article'",
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
        json!({"enabled":true,"maxTouchPoints":1}),
    )
    .await?;
    client.execute("const q=document.querySelector('#q');q.focus();q.value='category:eng';q.setSelectionRange(q.value.length,q.value.length);q.dispatchEvent(new Event('input',{bubbles:true}))",vec![]).await?;
    wait_for(
        client,
        "document.querySelector('.search-completion')?.textContent.toLowerCase().includes('engineering')",
    )
    .await?;
    let point=client.execute("const r=document.querySelector('.search-completion').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}",vec![]).await?;
    emulate(
        client,
        "Input.dispatchTouchEvent",
        json!({"type":"touchStart","touchPoints":[{"x":point["x"],"y":point["y"]}]}),
    )
    .await?;
    emulate(
        client,
        "Input.dispatchTouchEvent",
        json!({"type":"touchEnd","touchPoints":[]}),
    )
    .await?;
    wait_for(
        client,
        "document.querySelector('#q').value.trim()==='category:engineering'",
    )
    .await?;
    wait_for(client,"!document.querySelector('.search-completions:not([hidden])') && document.querySelector('#q').getAttribute('aria-expanded')==='false'").await?;

    for (directory, scope) in [
        ("sources/publisher.invalid/", "source:publisher.invalid"),
        ("categories/engineering/", "category:engineering"),
        ("tags/rust/", "tag:rust"),
    ] {
        client.goto(&format!("{}{directory}", fixture.base)).await?;
        wait_for(
            client,
            &format!("document.querySelector('#q')?.value.trim()==='{}'", scope),
        )
        .await?;
        anyhow::ensure!(
            client.current_url().await?.query().is_none(),
            "archive scope must not rewrite its unsearched route"
        );
        client
            .find(Locator::Css(".search-clear"))
            .await?
            .click()
            .await?;
        wait_for(
            client,
            "location.pathname==='/reader/' && document.querySelector('#q')?.value===''",
        )
        .await?;
    }

    for (directory, title, introduction, qualifier) in [
        (
            "categories/",
            "Categories",
            "Categories group sources by subject.",
            "category:engineering",
        ),
        (
            "sources/",
            "Sources",
            "Sources are the feeds and sites you follow.",
            "source:publisher.invalid",
        ),
        (
            "tags/",
            "Tags",
            "Tags describe topics attached to articles.",
            "tag:reading",
        ),
    ] {
        client.goto(&format!("{}{directory}", fixture.base)).await?;
        wait_for(client, "document.querySelector('.browse-entry-link')").await?;
        let headings = client.execute("return [...document.querySelectorAll('.browse-page h1,.browse-page h2')].map(e=>e.textContent.trim())", vec![]).await?;
        anyhow::ensure!(
            headings == json!([title]),
            "{directory} must have one heading without repeating the directory name: {headings}"
        );
        anyhow::ensure!(
            client
                .execute(
                    "return document.querySelector('.browse-intro').textContent",
                    vec![]
                )
                .await?
                .as_str()
                .is_some_and(|text| text.contains(introduction)),
            "{directory} explains its own organizing concept"
        );
        let link = if directory == "sources/" {
            ".browse-entry-link[href*=\"source%3A%22publisher.invalid%22\"]"
        } else {
            ".browse-entry-link"
        };
        canonical_facet_click(client, link, qualifier, 45).await?;
    }
    for (selector, qualifier, count) in [
        (".row .meta .domain > a", "source:publisher.invalid", 45),
        (".row .meta .category a", "category:engineering", 45),
    ] {
        client.goto(&fixture.base).await?;
        wait_for(
            client,
            &format!("document.querySelector({})", json!(selector)),
        )
        .await?;
        canonical_facet_click(client, selector, qualifier, count).await?;
    }
    client
        .goto(&format!(
            "{}items/example/2026-09-01-story-45/",
            fixture.base
        ))
        .await?;
    wait_for(client, "document.querySelector('.item-tags .tag')").await?;
    canonical_facet_click(client, ".item-tags .tag", "tag:reading", 45).await?;
    // The same two facets again, this time on a result the search itself rendered.
    for (selector, qualifier) in [
        (
            ".search-results .meta .domain > a",
            "source:publisher.invalid",
        ),
        (".search-results .meta .category a", "category:engineering"),
    ] {
        client
            .goto(&format!("{}?q=tag%3Areading", fixture.base))
            .await?;
        wait_for(
            client,
            &format!("document.querySelector({})", json!(selector)),
        )
        .await?;
        canonical_facet_click(client, selector, qualifier, 45).await?;
    }

    let preview_article = walkdir::WalkDir::new(fixture.out.join("items"))
        .into_iter()
        .filter_map(Result::ok)
        .find(|entry| {
            entry.file_type().is_file()
                && entry.file_name() == "index.html"
                && std::fs::read_to_string(entry.path())
                    .ok()
                    .and_then(|html| {
                        html.split_once("<footer class=\"article-footer\"")
                            .map(|(_, footer)| {
                                footer.contains("class=\"preview-image\"")
                                    && footer.matches("class=\"article-more-card").count() == 4
                            })
                    })
                    .unwrap_or(false)
        })
        .context("fixture article recommendation with an archived thumbnail")?;
    let relative = preview_article
        .path()
        .strip_prefix(&fixture.out)?
        .parent()
        .context("article directory")?
        .to_string_lossy();
    client.goto(&format!("{}{relative}/", fixture.base)).await?;
    wait_for(
        client,
        "document.querySelector('.article-more-card .preview-image')",
    )
    .await?;
    anyhow::ensure!(client.execute("const image=document.querySelector('.article-more-card .preview-image');const card=image.closest('.article-more-card');return image.width>0 && image.height>0 && image.getAttribute('loading')==='lazy' && ['.title','.meta .domain','.meta .category a','.meta .published-date','.meta .reading-stats','.meta .u-bookmark-of'].every(selector=>card.querySelector(selector))",vec![]).await?==true,"recommendation cards retain reserved thumbnails and full feed metadata");
    for width in [1280, 390] {
        emulate(
            client,
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":width<600}),
        )
        .await?;
        let layout=client.execute("const cards=[...document.querySelectorAll('.article-more-card')].map(card=>card.getBoundingClientRect());return {count:cards.length,sameRow:Math.abs(cards[0].top-cards[1].top)<=1,separateColumns:cards[1].left>=cards[0].right,stacked:cards[1].top>=cards[0].bottom,overflow:document.documentElement.scrollWidth>document.documentElement.clientWidth+1}",vec![]).await?;
        anyhow::ensure!(
            layout["count"] == 4 && layout["overflow"] == false,
            "recommendation cards fit the viewport at {width}px: {layout}"
        );
        anyhow::ensure!(
            layout["stacked"] == true,
            "recommendations stack vertically at {width}px: {layout}"
        );
    }

    Ok(())
}

/// Click a link that a live result list may re-render underneath us: the element found a moment
/// ago can be gone by the time the click lands, and the answer is simply to find it again.
async fn click_steadily(client: &Client, selector: &str) -> Result<()> {
    for attempt in 0..3 {
        match client.find(Locator::Css(selector)).await?.click().await {
            Ok(()) => return Ok(()),
            Err(error) if attempt < 2 && error.to_string().contains("stale") => {
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

async fn canonical_facet_click(
    client: &Client,
    selector: &str,
    qualifier: &str,
    count: usize,
) -> Result<()> {
    let (kind, value) = qualifier
        .split_once(':')
        .context("fixture facet qualifier")?;
    let qualifier = format!("{kind}:{}", json!(value));
    let url = client
        .execute(
            "return document.querySelector(arguments[0]).href",
            vec![json!(selector)],
        )
        .await?;
    let url = url::Url::parse(url.as_str().context("facet link URL")?)?;
    // A facet either has a collection page of its own, which opens scoped to it, or it is a
    // canonical feed query anyone can copy. Both land on the same articles.
    if url.query().is_none() {
        let path = url.path().to_string();
        click_steadily(client, selector).await?;
        wait_for(client, &format!("location.pathname==={} && document.querySelector('#q')?.value.trim()==={} && document.querySelectorAll('.rows .row').length > 0", json!(path), json!(format!("{kind}:{value}")))).await?;
        return Ok(());
    }
    anyhow::ensure!(
        url.path() == "/reader/"
            && url
                .query_pairs()
                .any(|(key, value)| key == "q" && value == qualifier),
        "facet href must be a copyable canonical feed query: {url}"
    );
    click_steadily(client, selector).await?;
    wait_for(client, &format!("location.pathname==='/reader/' && new URL(location.href).searchParams.get('q')==={} && document.querySelector('#q')?.value==={} && document.querySelector('#search-status')?.textContent==='{count} articles'",json!(qualifier),json!(qualifier))).await?;
    Ok(())
}
