//! Content and app deployments arriving while the reader is open.

use anyhow::Result;
use fantoccini::{Client, Locator};
use serde_json::{Value, json};

use crate::harness::{
    Fixture, browser_client, catch_panics, emulate, finish, report_failure, wait_booted,
    wait_booted_with, wait_for,
};

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn content_and_app_deployments_have_distinct_browser_behavior() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    for (width, installed) in [(1280, false), (390, true)] {
        let fixture = Fixture::with_pwa(installed)?;
        let client = browser_client().await?;
        let result = catch_panics(deployment_contracts(&client, &fixture, width, installed)).await;
        report_failure(&client, "deployment", &result).await;
        if result.is_err() {
            eprintln!("deployment width={width}, installed={installed}");
            eprintln!("deployment state: {}", client.execute_async(r#"
              const done=arguments[arguments.length-1];
              fetch(new URL('updates.json',document.baseURI),{cache:'no-store'}).then(response=>response.json()).then(latest=>done({
                rows:[...document.querySelectorAll('#list .row [data-row-open]')].map(link=>link.textContent),
                empty:document.querySelector('#empty')?.textContent,status:document.querySelector('#search-status')?.textContent,
                versions:window.manifestVersionsSeen,q:document.querySelector('#q')?.value,
                selected:document.querySelector('.row.is-selected [data-row-open]')?.href,
                focused:document.activeElement.outerHTML.slice(0,200),update:document.documentElement.dataset.updateState,
                initialApp:window.AGGR.appVersion,initialContent:window.AGGR.contentVersion,
                latestApp:latest.app_version,latestContent:latest.content_version,
                entryRequests:performance.getEntriesByType('resource').filter(entry=>entry.name.includes('pagefind-entry')).map(entry=>({name:entry.name,size:entry.transferSize}))
              }),error=>done(String(error)));
            "#, vec![]).await.unwrap_or(Value::Null));
        }
        finish(client, result).await?;
    }
    Ok(())
}

async fn deployment_contracts(
    client: &Client,
    fixture: &Fixture,
    width: u32,
    installed: bool,
) -> Result<()> {
    accelerate_update_polling(client).await?;
    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":installed}),
    )
    .await?;
    if installed {
        emulate(
            client,
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source":"Object.defineProperty(navigator,'standalone',{value:true,configurable:true})"}),
        )
        .await?;
    }
    client.goto(&fixture.base).await?;
    wait_booted(client).await?;
    if installed {
        wait_for(client, "!!navigator.serviceWorker.controller").await?;
        client
            .execute(
                "window.workerBeforeDeployment = navigator.serviceWorker.controller",
                vec![],
            )
            .await?;
    }
    client
        .execute(
            "window.swup.navigate(arguments[0])",
            vec![json!(format!("{}?q=article", fixture.base))],
        )
        .await?;
    wait_for(client, "document.querySelectorAll('#list .row').length > 0").await?;
    client
        .execute(
            "window.swup.navigate(arguments[0])",
            vec![json!(fixture.base)],
        )
        .await?;
    wait_for(
        client,
        "document.body.dataset.kind === 'river' && !window.swup.navigating",
    )
    .await?;
    client.execute(r#"
      window.deploymentSentinel = 42;
      document.dispatchEvent(new KeyboardEvent('keydown',{key:'j',bubbles:true}));
      document.querySelector('.row.is-selected [data-row-open]').focus({preventScroll:true});
      window.selectedBeforeDeployment = document.querySelector('.row.is-selected [data-row-open]').href;
      window.scrollBeforeDeployment = scrollY;
      window.manifestVersionsSeen = [];
      const fetchOriginal = window.fetch;
      window.fetch = function (...args) {
        return fetchOriginal.apply(this,args).then(response => {
          if (response.url.includes('/updates.json')) response.clone().json().then(value => window.manifestVersionsSeen.push(value.content_version));
          return response;
        });
      };
    "#, vec![]).await?;
    fixture.deploy_content_update(46)?;
    if installed {
        client.execute_async("const done=arguments[arguments.length-1];navigator.serviceWorker.getRegistration().then(registration=>registration.update()).then(()=>done(true),error=>done(error.message))", vec![]).await?;
        wait_for(
            client,
            "navigator.serviceWorker.controller !== window.workerBeforeDeployment",
        )
        .await?;
    }
    wait_for(
        client,
        "document.querySelector('.row [data-row-open]')?.textContent.includes('Freshly delivered article 46')",
    )
    .await?;
    let feed_state = client.execute(r#"
      return {
        sentinel: window.deploymentSentinel,
        selected: document.querySelector('.row.is-selected [data-row-open]')?.href === window.selectedBeforeDeployment,
        focused: document.activeElement?.href === window.selectedBeforeDeployment,
        scroll: scrollY === window.scrollBeforeDeployment,
        update: document.documentElement.dataset.updateState,
        pill: !!document.querySelector('#pwa-refresh') && !document.querySelector('#pwa-refresh').hidden
      };
    "#, vec![]).await?;
    assert_eq!(feed_state["sentinel"], 42, "{feed_state}");
    assert_eq!(feed_state["selected"], true, "{feed_state}");
    assert_eq!(feed_state["focused"], true, "{feed_state}");
    assert_eq!(feed_state["scroll"], true, "{feed_state}");
    assert_ne!(feed_state["update"], "ready", "{feed_state}");
    assert_eq!(feed_state["pill"], false, "{feed_state}");

    client
        .execute(
            "window.swup.navigate(arguments[0])",
            vec![json!(format!("{}?q=Freshly%20delivered", fixture.base))],
        )
        .await?;
    wait_for(
        client,
        "document.querySelector('#list .row [data-row-open]')?.textContent.includes('Freshly delivered article 46')",
    )
    .await?;
    wait_for(client, "!window.swup.navigating").await?;
    client
        .execute(
            r#"
      const row=document.querySelector('#list .row');
      row.classList.add('is-selected');
      const link=row.querySelector('[data-row-open]');
      link.focus({preventScroll:true});
      window.selectedSearchBeforeDeployment=link.href;
      window.searchInputBeforeDeployment=document.querySelector('#q');
    "#,
            vec![],
        )
        .await?;
    fixture.deploy_content_update(47)?;
    wait_for(
        client,
        "document.querySelectorAll('#list .row').length === 2",
    )
    .await?;
    assert_eq!(client.execute(r#"
      return window.deploymentSentinel === 42
        && document.querySelector('#list .row.is-selected [data-row-open]')?.href === window.selectedSearchBeforeDeployment
        && document.activeElement?.href === window.selectedSearchBeforeDeployment
        && document.querySelector('#q') === window.searchInputBeforeDeployment
        && document.querySelector('#q').value === 'Freshly delivered'
        && document.documentElement.dataset.updateState !== 'ready'
        && document.querySelector('#pwa-refresh').hidden;
    "#, vec![]).await?, true, "updating active search must preserve selected result, focus, and the current input");

    client
        .execute(
            "window.swup.navigate(arguments[0])",
            vec![json!(format!(
                "{}items/example/2026-09-01-story-45/",
                fixture.base
            ))],
        )
        .await?;
    wait_for(client, "!!document.querySelector('.body pre')").await?;
    client.execute("history.replaceState(history.state,'',location.pathname+'?reading=1#article');window.articleBeforeDeployment=document.querySelector('article.item');window.scrollTo(0,300)", vec![]).await?;
    let reading_url = client.current_url().await?;
    fixture.deploy_content_update(48)?;
    let content_version: Value =
        serde_json::from_slice(&std::fs::read(fixture.out.join("updates.json"))?)?;
    wait_for(
        client,
        &format!(
            "window.manifestVersionsSeen.includes({})",
            content_version["content_version"]
        ),
    )
    .await?;
    client.execute_async("const done=arguments[arguments.length-1];requestAnimationFrame(()=>requestAnimationFrame(()=>done(true)))", vec![]).await?;
    assert_eq!(client.execute("return window.deploymentSentinel === 42 && document.querySelector('article.item') === window.articleBeforeDeployment && scrollY === 300 && document.documentElement.dataset.updateState !== 'ready' && document.querySelector('#pwa-refresh').hidden", vec![]).await?, true);
    assert_eq!(client.current_url().await?, reading_url);

    fixture.deploy_app_update()?;
    wait_for(client, "document.documentElement.dataset.updateState === 'ready' && !document.querySelector('#pwa-refresh').hidden").await?;
    let app_state = client
        .execute(
            r#"
      const button=document.querySelector('#pwa-refresh'); const box=button.getBoundingClientRect();
      return {text:button.textContent.trim(),width:box.width,height:box.height,
        article:document.querySelector('article.item')===window.articleBeforeDeployment,
        sentinel:window.deploymentSentinel,scroll:scrollY};
    "#,
            vec![],
        )
        .await?;
    assert!(
        app_state["text"]
            .as_str()
            .is_some_and(|text| text.contains("Refresh to update")),
        "{app_state}"
    );
    assert!(
        app_state["width"].as_f64().unwrap_or_default() >= 44.0,
        "{app_state}"
    );
    assert!(
        app_state["height"].as_f64().unwrap_or_default() >= 44.0,
        "{app_state}"
    );
    assert_eq!(app_state["article"], true, "{app_state}");
    assert_eq!(app_state["sentinel"], 42, "{app_state}");
    assert_eq!(app_state["scroll"], 300, "{app_state}");
    if installed {
        client
            .find(Locator::Css("#pwa-refresh"))
            .await?
            .click()
            .await?;
    } else {
        client.refresh().await?;
    }
    wait_for(client, "document.title.includes('Updated reading room') && document.documentElement.dataset.updateState !== 'ready'").await?;
    assert_eq!(client.current_url().await?, reading_url);
    assert_eq!(
        client
            .execute("return window.deploymentSentinel || null", vec![])
            .await?,
        Value::Null
    );
    if installed {
        wait_for(client, "scrollY === 300").await?;
    }
    Ok(())
}

async fn accelerate_update_polling(client: &Client) -> Result<()> {
    emulate(
        client,
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":r#"
      const interval=window.setInterval;
      window.setInterval=function(callback,delay,...args){
        if(delay===60000 || delay===15000){
          return interval.call(this,function(...values){
            if(!window.pauseUpdatePolling) callback(...values);
          },250,...args);
        }
        return interval.call(this,callback,delay,...args);
      };
    "#}),
    )
    .await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn pending_app_release_does_not_block_automatic_feed_updates() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = pending_release_feed_contracts(&client, &fixture).await;
    report_failure(&client, "pending-release-feed", &result).await;
    finish(client, result).await
}

async fn pending_release_feed_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    let stale_feed = std::fs::read_to_string(fixture.out.join("index.html"))?;
    accelerate_update_polling(client).await?;
    client.goto(&fixture.base).await?;
    wait_booted(client).await?;
    client
        .execute(
            r#"
      window.liveFeedSentinel=42;
      document.dispatchEvent(new KeyboardEvent('keydown',{key:'j',bubbles:true}));
      const selected=document.querySelector('.row.is-selected [data-row-open]');
      selected.focus({preventScroll:true});window.liveFeedSelection=selected.href;
    "#,
            vec![],
        )
        .await?;
    fixture.deploy_app_update()?;
    wait_for(client,"document.documentElement.dataset.updateState === 'ready' && !document.querySelector('#pwa-refresh').hidden").await?;
    fixture.deploy_content_update(49)?;
    wait_for(client,"document.querySelector('.row [data-row-open]')?.textContent.includes('Freshly delivered article 49')").await?;
    anyhow::ensure!(client.execute(r#"
      return window.liveFeedSentinel===42 && document.documentElement.dataset.updateState==='ready'
        && !document.querySelector('#pwa-refresh').hidden
        && document.querySelector('.row.is-selected [data-row-open]')?.href===window.liveFeedSelection
        && document.activeElement?.href===window.liveFeedSelection;
    "#,vec![]).await?==true,"content should arrive automatically while the app update waits, preserving selection and focus");
    client
        .execute(
            r#"
      const fetch=window.fetch;
      let hold=true;
      window.fetch=async function(...args){
        const response=await fetch.apply(this,args);
        if(hold && response.url.includes('/updates.json')){
          hold=false;
          window.pauseUpdatePolling=true;
          return new Promise(resolve=>{
            window.releaseManifest=()=>resolve(response);
            window.manifestHeld=true;
          });
        }
        return response;
      };
    "#,
            vec![],
        )
        .await?;
    wait_for(client, "window.manifestHeld === true").await?;
    fixture.deploy_content_update(50)?;
    client
        .execute(
            r#"
      const event=new Event('aggr:build',{cancelable:true});
      window.dispatchEvent(event);
      window.buildEventHandled=event.defaultPrevented;
      window.releaseManifest();
    "#,
            vec![],
        )
        .await?;
    wait_for(client,"document.querySelector('.row [data-row-open]')?.textContent.includes('Freshly delivered article 50')").await?;
    anyhow::ensure!(
        client
            .execute(
                r#"
      return window.buildEventHandled && window.pauseUpdatePolling && window.liveFeedSentinel===42
        && document.documentElement.dataset.updateState==='ready'
        && !document.querySelector('#pwa-refresh').hidden;
    "#,
                vec![]
            )
            .await?
            == true,
        "a build notification during an older manifest request must trigger another check without waiting for the next poll"
    );
    client
        .execute(
            "window.swup.navigate(arguments[0])",
            vec![json!(format!(
                "{}items/example/2026-09-01-story-45/",
                fixture.base
            ))],
        )
        .await?;
    wait_for(
        client,
        "!!document.querySelector('article.item') && !window.swup.navigating",
    )
    .await?;
    client.execute(r#"
      const url=new URL(arguments[0]).pathname;
      window.swup.cache.set(url,{url,html:arguments[1]});
      window.swup.hooks.before('page:view',()=>{
        if(document.querySelector('#aggr-page')?.dataset.kind==='river'){
          window.staleFeedSeen=document.querySelector('.row [data-row-open]')?.href.includes('story-45/');
        }
      });
      window.pauseUpdatePolling=false;
      window.swup.navigate(url);
    "#,vec![json!(fixture.base),json!(stale_feed)]).await?;
    wait_for(client,"document.querySelector('.row [data-row-open]')?.textContent.includes('Freshly delivered article 50')").await?;
    anyhow::ensure!(
        client
            .execute(
                r#"
      return window.staleFeedSeen && window.liveFeedSentinel===42
        && document.documentElement.dataset.updateState==='ready';
    "#,
                vec![]
            )
            .await?
            == true,
        "returning to a stale cached feed must reconcile its displayed content version without a full reload"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn clearing_updated_search_reveals_the_latest_static_feed() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        accelerate_update_polling(&client).await?;
        emulate(&client,"Page.addScriptToEvaluateOnNewDocument",json!({"source": "(()=>{window.searchRequests=[];const originalFetch=window.fetch;window.fetch=async function(...args){try{const response=await originalFetch.apply(this,args);window.searchRequests.push({url:String(args[0]),status:response.status});return response}catch(error){window.searchRequests.push({url:String(args[0]),error:String(error)});throw error}}})()"})).await?;
        client.goto(&fixture.base).await?;
        wait_booted_with(&client, "document.querySelector('.search-command')").await?;
        client.execute(r#"
          window.searchDeploymentSentinel={};
          window.originalSearchDeploymentSentinel=window.searchDeploymentSentinel;
          const input=document.querySelector('#q');
          input.value='category:engineering';
          input.setSelectionRange(input.value.length,input.value.length);
          input.dispatchEvent(new Event('input',{bubbles:true}));
        "#,vec![]).await?;
        wait_for(&client,"document.querySelector('#search-status')?.textContent.trim()==='45 articles'").await?;
        fixture.deploy_content_update(49)?;
        wait_for(&client,"document.querySelector('#search-status')?.textContent.trim()==='46 articles' && document.querySelector('.search-results [data-row-open]')?.textContent.includes('Freshly delivered article 49')").await?;
        anyhow::ensure!(client.execute("return document.querySelector('#q').value==='category:engineering' && document.querySelector('[data-static-feed]').hidden",vec![]).await?==true,"the active search survives content delivery and keeps the static feed hidden");
        client.find(Locator::Css(".search-clear")).await?.click().await?;
        wait_for(&client,"!document.querySelector('[data-static-feed]').hidden && document.querySelector('[data-static-feed] [data-row-open]')?.textContent.includes('Freshly delivered article 49')").await?;
        anyhow::ensure!(client.execute(r#"
          return !!window.searchDeploymentSentinel && window.searchDeploymentSentinel===window.originalSearchDeploymentSentinel
            && !new URL(location.href).searchParams.has('q')
            && document.querySelector('#q').value===''
            && document.activeElement===document.querySelector('#q')
            && document.querySelectorAll('.search-command').length===1
            && document.querySelector('#pwa-refresh').hidden;
        "#,vec![]).await?==true,"clearing refreshed search shows current static content without reload, duplicate controls, lost input focus, or an app-update prompt");
        Ok(())
    }.await;
    if result.is_err() {
        eprintln!("search deployment diagnostics: {}",client.execute("return {error:document.querySelector('.search-error')?.textContent,requests:window.searchRequests,base:document.baseURI}",vec![]).await.unwrap_or(Value::Null));
    }
    report_failure(&client, "search-deployment-clear", &result).await;
    finish(client, result).await
}
