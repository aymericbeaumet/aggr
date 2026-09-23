//! Search: completion, results, keyboard selection and the offline index.

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
        "canonical search must keep Swup's history URL aligned with the address bar"
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
    wait_for(client, "document.querySelectorAll('#list .row').length === 25 && !!document.activeElement.matches('[data-row-open]')").await?;
    assert_eq!(
        client
            .execute("return document.activeElement.href", vec![])
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
                "Example": "source · 45", local_publisher: "source · 1",
                "twitch.tv": "source · 1", "vimeo.com": "source · 1", "youtube.com": "source · 1",
            })),
            ("category:news source:", "Underscore_", json!({"Underscore_": "source · 1"})),
            ("type:podcast source:", "Underscore_", json!({"Underscore_": "source · 1"})),
        ] {
            client.execute("const q=document.querySelector('#q');q.focus();q.value=arguments[0];q.setSelectionRange(q.value.length,q.value.length);q.dispatchEvent(new Event('input',{bubbles:true}))",vec![json!(query)]).await?;
            wait_for(&client,&format!("[...document.querySelectorAll('.completion-label')].some(label=>label.textContent==={})",json!(label))).await?;
            let completions = client.execute("return Object.fromEntries([...document.querySelectorAll('.search-completion')].map(entry=>[entry.querySelector('.completion-label').textContent,entry.querySelector('small').textContent]))",vec![]).await?;
            anyhow::ensure!(completions == expected, "{query}: publisher and feed counts reflect the other active clauses: {completions}, expected {expected}");
        }
        client.execute("const q=document.querySelector('#q');q.value='type:podcast source:under';q.setSelectionRange(q.value.length,q.value.length);q.dispatchEvent(new Event('input',{bubbles:true}))",vec![]).await?;
        wait_for(&client,"document.querySelectorAll('.search-completion').length===1 && document.querySelector('.completion-label')?.textContent==='Underscore_'").await?;
        key(&client,"Enter").await?;
        wait_for(&client,"document.querySelector('#q').value.includes('source:\"open.spotify.com\"') && !document.querySelector('.search-completions') && document.querySelector('#search-status')?.textContent==='1 article'").await?;
        anyhow::ensure!(client.execute("return document.querySelector('.search-results .title').textContent",vec![]).await?=="A podcast episode","Enter selects the named show");
        anyhow::ensure!(client.execute("return document.querySelector('.search-results .source-resolved').textContent",vec![]).await?=="open.spotify.com","podcast publisher metadata uses its canonical hostname");
        anyhow::ensure!(client.execute("return document.querySelectorAll('.search-results .source-feed').length",vec![]).await?==0,"same-host podcast publisher and feed share one source membership");
        search_query(&client,"source:youtube.com",1).await?;
        let publisher_hit = client.execute("return document.querySelector('.search-results .title').href",vec![]).await?;
        anyhow::ensure!(client.execute("return [...document.querySelectorAll('.search-results .domain a')].map(link=>link.textContent)",vec![]).await?==json!(["youtube.com", "publisher.invalid"]),"publisher and originating feed both have source links");
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
        key(&client,"j").await?;
        let stepped=client.execute("const rows=[...document.querySelectorAll('.search-results .row')];return {selected:rows.findIndex(row=>row.classList.contains('is-selected')),focused:document.activeElement.id}",vec![]).await?;
        anyhow::ensure!(stepped["selected"].as_i64().unwrap_or(-1)>=1 && stepped["focused"]!="q","j walks the search results once the field is left: {stepped}");
        key(&client,"k").await?;
        anyhow::ensure!(client.execute("return [...document.querySelectorAll('.search-results .row')].findIndex(row=>row.classList.contains('is-selected'))",vec![]).await?==0,"k walks back up the search results");
        // A half-typed qualifier is completed in two steps: the name, then the values it accepts.
        client.execute("const q=document.querySelector('#q');q.value='Article sour';q.setSelectionRange(12,12);q.dispatchEvent(new Event('input',{bubbles:true}))",vec![]).await?;
        wait_for(&client,"document.querySelector('.search-completion')?.dataset.completionId==='source:'").await?;
        key(&client,"Enter").await?;
        wait_for(&client,"document.querySelector('#q').value==='Article source:' && [...document.querySelectorAll('.search-completion')].some(row=>row.dataset.completionId.startsWith('source:'))").await?;
        client.execute("const q=document.querySelector('#q');q.value='source:';q.dispatchEvent(new Event('input',{bubbles:true}))",vec![]).await?;
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
            anyhow::ensure!(client.execute("return typeof window.Swup==='undefined' && !document.querySelector('.search-command')",vec![]).await?==true,"scripts stay blocked while sampling the styled static field");
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
          window.fetch=async function(input,...rest){
            if(String(input?.url||input).includes('search-catalog.json')) await new Promise(resolve=>{window.releaseSearchManifest=resolve});
            return originalFetch.call(this,input,...rest);
          };
        "#})).await?;
        client.goto(&fixture.base).await?;
        wait_for(&client,"document.querySelector('.search-input-line')").await?;
        client.find(Locator::Css("#q")).await?.send_keys("source:").await?;
        wait_for(&client,"typeof window.releaseSearchManifest==='function'").await?;
        anyhow::ensure!(client.execute("return document.querySelector('[data-static-feed]').hidden && !document.querySelector('.search-results .row')",vec![]).await?==true,"feed rows stay hidden while the search catalogue loads");
        client.execute("window.releaseSearchManifest()",vec![]).await?;
        wait_for(&client,"[...document.querySelectorAll('.search-completion')].some(row=>row.textContent.toLowerCase().includes('example'))").await?;
        key(&client,"\u{e015}").await?;
        key(&client,"\u{e013}").await?;
        key(&client,"Tab").await?;
        wait_for(&client,"document.querySelector('#q').value.trim()==='source:\"publisher.invalid\"' && !document.querySelector('.search-completions')").await?;
        wait_for(&client,"document.querySelector('.search-results .row')").await?;
        Ok(())
    }.await;
    fixture.scripts_blocked.store(false, Ordering::Relaxed);
    report_failure(&client, "search-control-mount", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn search_focus_scrolls_only_when_obscured() -> Result<()> {
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
                client.execute("window.scrollTo({top:650,behavior:'instant'})",vec![]).await?;
                wait_for(&client,"scrollY>500").await?;
                client.execute(&format!("document.dispatchEvent(new KeyboardEvent('keydown',{{key:'k',{modifier}:true,bubbles:true,cancelable:true}}))"),vec![]).await?;
                wait_for(&client,"scrollY===0 && document.activeElement.id==='q'").await?;
            }
            client.execute("document.querySelector('#q').blur();const q=document.querySelector('#q').getBoundingClientRect(),header=document.querySelector('.top').getBoundingClientRect();window.scrollTo({top:q.top-header.bottom+q.height/2,behavior:'instant'})",vec![]).await?;
            let point=client.execute("const q=document.querySelector('#q').getBoundingClientRect();return {x:q.left+20,y:q.bottom-4}",vec![]).await?;
            emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mousePressed","x":point["x"],"y":point["y"],"button":"left","clickCount":1})).await?;
            emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mouseReleased","x":point["x"],"y":point["y"],"button":"left","clickCount":1})).await?;
            wait_for(&client,"scrollY===0 && document.activeElement.id==='q'").await?;
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
async fn rich_search_and_complete_offline_index() -> Result<()> {
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
        eprintln!("search state: {}", client.execute("return {error:document.querySelector('.search-error')?.textContent,offline:window.searchOfflineStatus,rows:document.querySelectorAll('.search-results .row').length}", vec![]).await.unwrap_or(Value::Null));
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
    client.execute("const input=document.querySelector('#q');input.focus();input.value=arguments[0];input.setSelectionRange(input.value.length,input.value.length);input.dispatchEvent(new Event('input',{bubbles:true}));document.querySelector('#search-form').dispatchEvent(new Event('submit',{bubbles:true,cancelable:true}))", vec![json!(query)]).await?;
    wait_for(client, &format!("document.querySelector('#search-status')?.textContent === '{} {}' && !document.querySelector('.search-error')", count, if count == 1 { "article" } else { "articles" })).await?;
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
    client.execute("window.searchOfflineStatus=null;navigator.serviceWorker.addEventListener('message',event=>{if(event.data?.type==='AGGR_OFFLINE_STATUS')window.searchOfflineStatus=event.data});navigator.serviceWorker.controller.postMessage({type:'AGGR_OFFLINE_GET_STATUS'})", vec![]).await?;
    wait_for(client, "window.searchOfflineStatus?.search?.phase==='ready' && window.searchOfflineStatus.saved?.length===4").await?;
    let ready = client
        .execute("return window.searchOfflineStatus.search", vec![])
        .await?;
    anyhow::ensure!(
        ready["downloadedFiles"] == ready["totalFiles"]
            && ready["downloadedBytes"] == ready["totalBytes"],
        "ready means the entire index, not only its runtime, is saved"
    );
    let complete=client.execute_async("const done=arguments[arguments.length-1];(async()=>{const m=await(await fetch('search-manifest.json')).json();const c=await caches.open('aggr:'+encodeURIComponent('/reader/')+':offline-search-'+m.version);const keys=new Set((await c.keys()).map(k=>k.url));return m.docs===45&&m.files.every(f=>keys.has(new URL(f.url,location.href).href))})().then(done)",vec![]).await?;
    anyhow::ensure!(
        complete == true,
        "every index shard must exist before readiness"
    );
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
    let hover=client.execute("const r=document.querySelector('#q').getBoundingClientRect();return {x:r.left+20,y:r.top+r.height/2}",vec![]).await?;
    emulate(
        client,
        "Input.dispatchMouseEvent",
        json!({"type":"mouseMoved","x":hover["x"],"y":hover["y"]}),
    )
    .await?;
    wait_for(client,"getComputedStyle(document.querySelector('#search-query-help')).display!=='none' && document.querySelector('#search-query-help').getAttribute('role')==='tooltip'").await?;
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
    wait_for(
        client,
        "getComputedStyle(document.querySelector('#search-query-help')).display!=='none'",
    )
    .await?;
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
    key(client, "Escape").await?;
    wait_for(client,"getComputedStyle(document.querySelector('#search-query-help')).display==='none' && document.querySelector('#q').getAttribute('aria-expanded')==='false'").await?;

    // The first query occurs after going offline: no query or result fragment was warmed online.
    fixture.offline.store(true, Ordering::Relaxed);
    set_offline(client, true).await?;
    search_query(client, "\"cobalt marmalade\"", 1).await?;
    wait_for(
        client,
        "document.querySelector('.search-results [data-row-open]') && !document.querySelector('.search-results .search-saved-status')",
    )
    .await?;
    anyhow::ensure!(
        client
            .execute(
                "return document.querySelector('.search-results [data-row-open]').textContent",
                vec![]
            )
            .await?
            == "Article 1",
        "fresh offline full-text search must find an old, unsaved article"
    );
    search_query(client, "\"rare exclusion\"", 2).await?;
    wait_for(
        client,
        "document.querySelectorAll('.search-results [data-saved-offline=true]').length===2",
    )
    .await?;
    fixture.offline.store(false, Ordering::Relaxed);
    set_offline(client, false).await?;

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
            "[data-search-results] .pager button:last-child",
        ))
        .await?
        .click()
        .await?;
    wait_for(client, "document.querySelectorAll('.search-results .row').length===18 && document.querySelector('.search-results .rank')?.textContent==='26.'").await?;
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
    wait_for(client,"Array.from(document.querySelectorAll('.search-completion')).some(e=>e.textContent.includes('Example'))").await?;
    client
        .active_element()
        .await?
        .send_keys("\u{e015}\u{e007}")
        .await?;
    wait_for(client,"document.querySelector('#q').value.includes('source:\"publisher.invalid\"') && document.querySelector('#q').value.includes('tag:rust')").await?;
    anyhow::ensure!(
        client
            .execute("return document.activeElement?.id", vec![])
            .await?
            == "q",
        "completion retains editor focus"
    );
    wait_for(client,"!document.querySelector('.search-completions') && document.querySelector('#q').getAttribute('aria-expanded')==='false'").await?;
    client.execute("const q=document.querySelector('#q');q.value='sort:';q.setSelectionRange(5,5);q.dispatchEvent(new Event('input',{bubbles:true}))",vec![]).await?;
    wait_for(
        client,
        "document.querySelectorAll('.search-completion').length===3",
    )
    .await?;
    // Selection moves asynchronously, so each arrow waits for a different highlighted label
    // instead of sampling whatever the previous render left behind.
    let selected_label = "document.querySelector('.search-completion[data-selected] .completion-label')?.textContent";
    let initial_choice = client
        .execute(&format!("return {selected_label}"), vec![])
        .await?;
    client.active_element().await?.send_keys("\u{e015}").await?;
    wait_for(client, &format!("(()=>{{const label={selected_label};return typeof label==='string' && label!=={initial_choice}}})()")).await?;
    let first_choice = client
        .execute(&format!("return {selected_label}"), vec![])
        .await?;
    client.active_element().await?.send_keys("\u{e015}").await?;
    wait_for(client, &format!("(()=>{{const label={selected_label};return typeof label==='string' && label!=={first_choice}}})()")).await?;
    let second_choice = client
        .execute(&format!("return {selected_label}"), vec![])
        .await?;
    anyhow::ensure!(
        first_choice.is_string() && second_choice.is_string() && first_choice != second_choice,
        "successive arrows must advance selection without keyup resetting it"
    );
    key(client, "Enter").await?;
    wait_for(client,&format!("document.querySelector('#q').value.trim()==={} && !document.querySelector('.search-completions')",json!(format!("sort:{}",second_choice.as_str().unwrap_or_default())))).await?;
    wait_for(
        client,
        "document.querySelector('#search-status')?.textContent==='45 articles'",
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(250)).await;
    anyhow::ensure!(client.execute("return !document.querySelector('.search-completions') && document.querySelector('#q').getAttribute('aria-expanded')==='false'",vec![]).await?==true,"selected completion must stay closed when results arrive");

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
    wait_for(client,"!document.querySelector('.search-completions') && document.querySelector('#q').getAttribute('aria-expanded')==='false'").await?;

    for (directory, scope) in [
        ("sources/publisher.invalid/", "source:publisher.invalid"),
        ("categories/engineering/", "category:engineering"),
        ("tags/rust/", "tag:rust"),
    ] {
        client.goto(&format!("{}{directory}", fixture.base)).await?;
        wait_for(
            client,
            &format!("document.querySelector('#q')?.value==='{}'", scope),
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
    canonical_facet_click(
        client,
        ".search-results .meta .domain > a",
        "source:publisher.invalid",
        45,
    )
    .await?;
    canonical_facet_click(
        client,
        ".search-results .meta .category a",
        "category:engineering",
        45,
    )
    .await?;

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

    client
        .goto(&format!("{}preferences/", fixture.base))
        .await?;
    wait_for(client, "document.querySelector('#offline-items')").await?;
    client.execute("window.searchOfflineStatus=null;navigator.serviceWorker.addEventListener('message',e=>{if(e.data?.type==='AGGR_OFFLINE_STATUS')window.searchOfflineStatus=e.data});const input=document.querySelector('#offline-items');input.value='1';input.dispatchEvent(new Event('change',{bubbles:true}))",vec![]).await?;
    wait_for(client,"window.searchOfflineStatus?.saved?.length===1 && window.searchOfflineStatus?.search?.phase==='ready'").await?;
    anyhow::ensure!(
        client
            .execute(
                "return window.searchOfflineStatus.search.activeVersion",
                vec![]
            )
            .await?
            == ready["activeVersion"],
        "changing article count keeps the same complete search index"
    );
    // A deployment can expose a new online index before its worker/index replacement succeeds.
    std::fs::write(
        fixture.directory.path().join(".block-worker-updates"),
        b"503",
    )?;
    fixture.deploy_content_update(46)?;
    client
        .goto(&format!(
            "{}?q=%22Newly%20delivered%20reading%22",
            fixture.base
        ))
        .await?;
    wait_for(
        client,
        "document.querySelector('#search-status')?.textContent==='1 article'",
    )
    .await?;
    anyhow::ensure!(
        client
            .execute(
                "return document.querySelector('.search-results [data-row-open]').textContent",
                vec![]
            )
            .await?
            == "Freshly delivered article 46",
        "the online reader must have loaded the new deployment's index"
    );
    client.execute("window.searchOfflineStatus=null;navigator.serviceWorker.addEventListener('message',e=>{if(e.data?.type==='AGGR_OFFLINE_STATUS')window.searchOfflineStatus=e.data});navigator.serviceWorker.controller.postMessage({type:'AGGR_OFFLINE_GET_STATUS'})",vec![]).await?;
    wait_for(
        client,
        "window.searchOfflineStatus?.search?.phase==='ready'",
    )
    .await?;
    anyhow::ensure!(
        client
            .execute(
                "return window.searchOfflineStatus.search.activeVersion",
                vec![]
            )
            .await?
            == ready["activeVersion"],
        "the complete old index remains committed while the online reader loads the new one"
    );
    fixture.offline.store(true, Ordering::Relaxed);
    set_offline(client, true).await?;
    wait_for(
        client,
        "document.querySelector('#search-status')?.textContent==='0 articles'",
    )
    .await?;
    search_query(client, "\"cobalt marmalade\"", 1).await?;
    anyhow::ensure!(client.execute("return document.querySelector('.search-results [data-row-open]')!==null && document.querySelector('.search-results .search-saved-status')===null",vec![]).await?==true,"falling back to the older complete index does not mark uncached articles saved");
    client
        .goto(&format!("{}preferences/", fixture.base))
        .await?;
    wait_for(client, "document.querySelector('#offline-items')").await?;
    client.execute("window.searchOfflineStatus=null;navigator.serviceWorker.addEventListener('message',e=>{if(e.data?.type==='AGGR_OFFLINE_STATUS')window.searchOfflineStatus=e.data});navigator.serviceWorker.controller.postMessage({type:'AGGR_OFFLINE_GET_STATUS'})",vec![]).await?;
    wait_for(
        client,
        "window.searchOfflineStatus?.search?.phase==='ready'",
    )
    .await?;
    anyhow::ensure!(
        client
            .execute(
                "return window.searchOfflineStatus.search.activeVersion",
                vec![]
            )
            .await?
            == ready["activeVersion"],
        "the online-to-offline transition selects the previously committed index"
    );
    client.execute("window.zeroStatusRegressions=[];window.sawDisabledStatus=false;navigator.serviceWorker.addEventListener('message',event=>{const status=event.data;if(status?.type!=='AGGR_OFFLINE_STATUS')return;if((status.requested===0 && status.search?.phase!=='disabled') || (window.sawDisabledStatus && status.requested!==0))window.zeroStatusRegressions.push(status);if(status.requested===0 && status.search?.phase==='disabled')window.sawDisabledStatus=true});const worker=navigator.serviceWorker.controller;for(let i=0;i<8;i++)worker.postMessage({type:'AGGR_OFFLINE_GET_STATUS'});const input=document.querySelector('#offline-items');input.value='0';input.dispatchEvent(new Event('change',{bubbles:true}));for(let i=0;i<8;i++)worker.postMessage({type:'AGGR_OFFLINE_GET_STATUS'})",vec![]).await?;
    wait_for(client,"window.searchOfflineStatus?.requested===0 && window.searchOfflineStatus?.search?.phase==='disabled'").await?;
    let empty=client.execute_async("const done=arguments[arguments.length-1];caches.keys().then(keys=>done(!keys.some(k=>k.includes(':offline-search-'))))",vec![]).await?;
    anyhow::ensure!(
        empty == true,
        "disabling offline saving removes every protected index generation"
    );
    client
        .execute_async(
            "const done=arguments[arguments.length-1];setTimeout(done,250)",
            vec![],
        )
        .await?;
    anyhow::ensure!(
        client
            .execute("return window.zeroStatusRegressions", vec![])
            .await?
            == json!([]),
        "concurrent read-only status replies must not resurrect ready search after N=0"
    );
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
    anyhow::ensure!(
        url.path() == "/reader/"
            && url
                .query_pairs()
                .any(|(key, value)| key == "q" && value == qualifier),
        "facet href must be a copyable canonical feed query: {url}"
    );
    client.find(Locator::Css(selector)).await?.click().await?;
    wait_for(client, &format!("location.pathname==='/reader/' && new URL(location.href).searchParams.get('q')==={} && document.querySelector('#q')?.value==={} && document.querySelector('#search-status')?.textContent==='{count} articles'",json!(qualifier),json!(qualifier))).await?;
    Ok(())
}
