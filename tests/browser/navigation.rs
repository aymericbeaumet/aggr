//! Swup navigation: prefetch sharing and cancellation, provider facades, component lifecycles.

use anyhow::{Context as _, Result};
use fantoccini::{Client, Locator};
use serde_json::{Value, json};
use sha1::{Digest as _, Sha1};

use crate::harness::{
    Fixture, browser_client, catch_panics, emulate, finish, git, key, phone_session,
    report_failure, screenshot, wait_booted_with, wait_for,
};

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn article_boundary_keys_and_swipes_return_to_feed() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let archive = fixture.directory.path().join(".aggr/data");
    for (index, date, title, body) in [
        (
            45,
            "2026-09-09",
            "Newest boundary article",
            "The latest dispatch covers navigation at the beginning of the reading sequence. Continue across this entire paragraph to return to the feed.",
        ),
        (
            1,
            "2026-08-01",
            "Oldest boundary article",
            "An earlier essay examines how a reader reaches the archive boundary after browsing the collection. A deliberate gesture beyond this essay returns to the article list.",
        ),
    ] {
        std::fs::write(
            archive.join(format!(
                "items/example/2026/09/2026-09-01-story-{index:02}.md"
            )),
            format!(
                "---\ntitle: {title}\nlink: https://publisher.invalid/boundary-{index}\nsource: example\npublished: {date}T12:00:00Z\nfirst_seen: {date}T12:00:00Z\ncontent: extracted\n---\n\n{body}\n"
            ),
        )?;
    }
    git(&archive, &["add", "items"])?;
    git(&archive, &["commit", "-qm", "article boundary fixture"])?;
    fixture.build()?;
    let client = browser_client().await?;
    let result = async {
        phone_session(&client).await?;
        for (index, direction, input) in [(45, "previous", "k"), (1, "next", "j"), (45, "previous", "swipe"), (1, "next", "swipe")] {
            client.goto(&format!("{}items/example/2026-09-01-story-{index:02}/", fixture.base)).await?;
            wait_booted_with(&client, "document.querySelector('article.item[data-swipe-navigation] .body p')").await?;
            let missing = client.execute("return !document.querySelector('article.item').dataset[arguments[0]+'Url']", vec![json!(direction)]).await?;
            anyhow::ensure!(missing == true, "{index} must be the real {direction} archive boundary");
            client.execute("window.__boundaryNavigation=true;document.activeElement?.blur()", vec![]).await?;
            if input == "swipe" {
                let y = client.execute("const p=document.querySelector('.body p');p.scrollIntoView({block:'center'});const box=p.getBoundingClientRect();return (box.top+box.bottom)/2", vec![]).await?.as_f64().context("article gesture height")?;
                let (from, to) = if direction == "previous" { (85.0, 300.0) } else { (300.0, 85.0) };
                emulate(&client, "Input.dispatchTouchEvent", json!({"type":"touchStart","touchPoints":[{"x":from,"y":y,"id":1}]})).await?;
                for step in 1..=8 {
                    let x = from + (to-from) * f64::from(step) / 8.0;
                    emulate(&client, "Input.dispatchTouchEvent", json!({"type":"touchMove","touchPoints":[{"x":x,"y":y,"id":1}]})).await?;
                }
                emulate(&client, "Input.dispatchTouchEvent", json!({"type":"touchEnd","touchPoints":[]})).await?;
            } else {
                key(&client, input).await?;
            }
            wait_for(&client, &format!("!window.swup.navigating && location.href==={} && document.body.dataset.kind==='river'", json!(fixture.base))).await?;
            anyhow::ensure!(client.execute("return window.__boundaryNavigation", vec![]).await? == true, "boundary navigation must preserve the document");
        }
        Ok(())
    }.await;
    report_failure(&client, "article-boundary-navigation", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn prefetch_sharing_and_video_provider_facades() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::new()?;
    let client = browser_client().await?;
    let result = catch_panics(prefetch_and_video_contracts(&client, &fixture)).await;
    report_failure(&client, "prefetch-and-video", &result).await;
    finish(client, result).await
}

async fn prefetch_and_video_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    phone_session(client).await?;
    client.goto(&fixture.base).await?;
    wait_booted_with(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3",
    )
    .await?;
    assert_eq!(client.execute(r#"
      const cached = window.swup.cache.get(location.href);
      return !!cached && !new DOMParser().parseFromString(cached.html,'text/html').querySelector('#swup [data-bound]');
    "#, vec![]).await?, true, "the initial page must be cached before event-binding markers are added");
    let shared_fetch = client.execute_async(r#"
      const done = arguments[arguments.length-1];
      const target = new URL('browse/?prefetch-contract=1',new URL(document.getElementById('aggr-page').dataset.root,location.href));
      Promise.all([window.swup.fetchPage(target.href), window.swup.fetchPage(target.pathname+target.search)]).then(pages => {
        done({requests:performance.getEntriesByName(target.href).length, sameHtml:pages[0].html===pages[1].html});
      }).catch(error => done({error:String(error)}));
    "#, vec![]).await?;
    assert_eq!(
        shared_fetch,
        json!({"requests":1,"sameHtml":true}),
        "concurrent prefetch/navigation requests should share one download"
    );
    client.execute(r#"
      const state = {
        originalFetch:window.fetch, home:location.href,
        destination:new URL('browse/?prefetch-cancellation=destination',new URL(document.getElementById('aggr-page').dataset.root,location.href)).href,
        unrelated:new URL('preferences/?prefetch-cancellation=unrelated',new URL(document.getElementById('aggr-page').dataset.root,location.href)).href,
        requests:{}, aborted:[], release:{}, settled:{}
      };
      window.prefetchCancellationContract = state;
      window.fetch = function(input, options) {
        const url = new URL(typeof input === 'string' ? input : input.url, new URL(document.getElementById('aggr-page').dataset.root,location.href));
        const key = url.searchParams.get('prefetch-cancellation');
        if (!key) return state.originalFetch.call(window,input,options);
        state.requests[key] = (state.requests[key] || 0) + 1;
        return new Promise((resolve,reject) => {
          const signal = options.signal;
          const abort = () => {
            state.aborted.push(key);
            reject(new DOMException('Canceled','AbortError'));
          };
          if (signal.aborted) { abort(); return; }
          signal.addEventListener('abort',abort,{once:true});
          state.release[key] = () => {
            state.originalFetch.call(window,input,options).then(resolve,reject).finally(() => signal.removeEventListener('abort',abort));
          };
        });
      };
      for (const key of ['destination','unrelated']) {
        window.swup.fetchPage(state[key],{priority:'low'}).then(
          () => { state.settled[key] = 'fulfilled'; },
          () => { state.settled[key] = 'rejected'; }
        );
      }
    "#, vec![]).await?;
    wait_for(
        client,
        "Object.keys(window.prefetchCancellationContract.requests).length === 2",
    )
    .await?;
    client
        .execute(
            "window.swup.navigate(window.prefetchCancellationContract.destination)",
            vec![],
        )
        .await?;
    wait_for(
        client,
        "window.prefetchCancellationContract.settled.unrelated === 'rejected'",
    )
    .await?;
    assert_eq!(client.execute(r#"
      const state = window.prefetchCancellationContract;
      return {requests:state.requests,aborted:state.aborted,destinationPending:!state.settled.destination};
    "#, vec![]).await?, json!({
        "requests":{"destination":1,"unrelated":1},
        "aborted":["unrelated"],
        "destinationPending":true
    }), "navigation must cancel unrelated speculation while retaining its shared destination request");
    client
        .execute(
            "window.prefetchCancellationContract.release.destination()",
            vec![],
        )
        .await?;
    wait_for(client, "document.body.dataset.kind === 'browse' && window.prefetchCancellationContract.settled.destination === 'fulfilled'").await?;
    assert_eq!(
        client
            .execute(
                "return window.prefetchCancellationContract.requests.destination",
                vec![]
            )
            .await?,
        1,
        "the destination must not download again when speculation becomes navigation"
    );
    client
        .execute(
            r#"
      const state = window.prefetchCancellationContract;
      window.fetch = state.originalFetch;
      window.swup.cache.delete(state.destination);
      window.swup.cache.delete(state.unrelated);
      window.swup.navigate(state.home);
    "#,
            vec![],
        )
        .await?;
    wait_for(client, "location.href === window.prefetchCancellationContract.home && document.body.dataset.kind === 'river' && document.querySelectorAll('[data-row-open]').length === 3").await?;
    client
        .execute("delete window.prefetchCancellationContract", vec![])
        .await?;
    for (index, provider, host) in [
        (44, "youtube", "www.youtube-nocookie.com"),
        (43, "twitch", "player.twitch.tv"),
        (42, "vimeo", "player.vimeo.com"),
    ] {
        client
            .goto(&format!(
                "{}items/example/2026-09-01-story-{index}/",
                fixture.base
            ))
            .await?;
        // Every provider, YouTube included, stays a facade until activation.
        wait_for(
            client,
            "document.querySelector('[data-video-embed]')?.getAttribute('role') === 'button'",
        )
        .await?;
        if provider == "youtube" {
            let source_colors = client.execute(r#"
              const source=document.querySelector('.itemhead .domain:has(em)'), span=source.querySelector('.source-resolved');
              const probe=document.createElement('span');probe.style.color='var(--warm)';document.body.append(probe);
              const result={source:getComputedStyle(span).color,warm:getComputedStyle(probe).color,via:getComputedStyle(source.querySelector('em')).color,neutral:getComputedStyle(source).color};probe.remove();return result;
            "#, vec![]).await?;
            assert_eq!(source_colors["source"], source_colors["warm"]);
            assert_eq!(source_colors["via"], source_colors["neutral"]);
            assert_ne!(source_colors["source"], source_colors["via"]);
            assert_eq!(client.execute("const date=document.querySelector('.published-date');date.dispatchEvent(new PointerEvent('pointerover',{bubbles:true}));return {duplicate:date.title.includes('Updated:'),separate:date.hasAttribute('data-date-updated')}", vec![]).await?, json!({"duplicate":false,"separate":false}), "identical published and updated dates should appear only once in the tooltip");
        }
        wait_for(
            client,
            "document.querySelector('[data-video-embed]')?.getAttribute('role') === 'button'",
        )
        .await?;
        assert_eq!(client.execute("return {frames:document.querySelectorAll('iframe').length,providerRequests:performance.getEntriesByType('resource').filter(r=>/youtube|twitch|vimeo/.test(new URL(r.name).hostname)).length}",vec![]).await?,json!({"frames":0,"providerRequests":0}),"video providers must not receive requests before activation");
        client.execute(r#"
      const player=document.querySelector('.video-player'), append=player.appendChild;
      player.appendChild=function(frame){
        window.videoContract={url:frame.src,allow:frame.allow,sandbox:frame.getAttribute('sandbox'),referrer:frame.referrerPolicy,title:frame.title};
        frame.removeAttribute('src');
        return append.call(this,frame);
      };
    "#,vec![]).await?;
        client
            .find(Locator::Css("[data-video-embed]"))
            .await?
            .click()
            .await?;
        let video = client.execute(r#"
          const frame=document.querySelector('.video-player iframe'), url=new URL(window.videoContract.url);
          return {...window.videoContract,host:url.hostname,parent:url.searchParams.get('parent'),autoplay:url.searchParams.get('autoplay'),rel:url.searchParams.get('rel'),dnt:url.searchParams.get('dnt'),h:url.searchParams.get('h'),frames:document.querySelectorAll('iframe').length,width:frame.clientWidth,height:frame.clientHeight,overflow:document.documentElement.scrollWidth>innerWidth};
        "#,vec![]).await?;
        assert_eq!(video["host"], host, "{video}");
        assert_eq!(video["frames"], 1);
        assert_eq!(video["referrer"], "strict-origin-when-cross-origin");
        assert_eq!(
            video["allow"],
            "autoplay; encrypted-media; fullscreen; picture-in-picture"
        );
        assert_eq!(
            video["sandbox"],
            "allow-scripts allow-same-origin allow-presentation"
        );
        assert!(video["title"].as_str().is_some_and(|s| s.contains("Play")));
        assert_eq!(video["overflow"], false, "{video}");
        if provider == "twitch" {
            assert_eq!(video["parent"], "127.0.0.1");
            assert_eq!(video["autoplay"], "true");
            assert!(
                video["width"].as_f64().unwrap_or_default() >= 400.0
                    && video["height"].as_f64().unwrap_or_default() >= 300.0,
                "{video}"
            );
        } else {
            assert_eq!(video["autoplay"], "1");
            assert_eq!(video["parent"], Value::Null);
        }
        if provider == "youtube" {
            assert_eq!(video["rel"], "0");
        }
        if provider == "vimeo" {
            assert_eq!(video["dnt"], "1");
            assert_eq!(video["h"], "abc123def4");
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn prefetch_reserves_capacity_for_pointer_intent() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        emulate(&client, "Page.addScriptToEvaluateOnNewDocument", json!({"source": r#"
          window.prefetchProbe={started:[],release:[]};
          const original=window.fetch;
          window.fetch=function(input,options){
            if(options?.priority!=='low')return original.call(this,input,options);
            const url=new URL(typeof input==='string'?input:input.url,new URL(document.getElementById('aggr-page').dataset.root,location.href)).href;
            window.prefetchProbe.started.push(url);
            return new Promise(resolve=>window.prefetchProbe.release.push(resolve)).then(()=>original.call(this,input,options));
          };
        "#})).await?;
        client.goto(&fixture.base).await?;
        wait_for(&client,"window.prefetchProbe.started.length>0").await?;
        let started=client.execute("const probe=window.prefetchProbe,idle=probe.started.length;const link=[...document.querySelectorAll('[data-row-open]')].at(-1);link.dispatchEvent(new PointerEvent('pointerover',{bubbles:true}));return {idle,target:link.href}",vec![]).await?;
        wait_for(&client,"window.prefetchProbe.started.length===2").await?;
        let requests=client.execute("return window.prefetchProbe.started",vec![]).await?;
        anyhow::ensure!(started["idle"]==1 && requests[1]==started["target"],"one idle request leaves room for immediate pointer intent: started={started}, requests={requests}");
        client.execute("window.prefetchProbe.release.forEach(resolve=>resolve())",vec![]).await?;
        Ok(())
    }.await;
    report_failure(&client, "prefetch-capacity", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn svelte_components_preserve_reader_lifecycles() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    for base_path in ["", "reader/"] {
        let fixture = Fixture::with_base_path(false, base_path)?;
        let archive = fixture.directory.path().join(".aggr/data");
        let path = archive.join("items/example/2026/09/2026-09-01-story-36.md");
        let cover = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            256,
            256,
            image::Rgb([192, 116, 72]),
        ));
        let mut cover_bytes = std::io::Cursor::new(Vec::new());
        cover.write_to(&mut cover_bytes, image::ImageFormat::Jpeg)?;
        let cover_bytes = cover_bytes.into_inner();
        let cover_hash = hex::encode(Sha1::digest(&cover_bytes));
        let cover_name = format!("2026-09-01-story-36.preview-{}.jpg", &cover_hash[..12]);
        std::fs::write(
            path.parent()
                .context("audio fixture directory")?
                .join(&cover_name),
            cover_bytes,
        )?;
        let text = std::fs::read_to_string(&path)?.replace(
            "content: feed\n",
            &format!(
                "content: feed\npreview:\n  file: {cover_name}\n  width: 256\n  height: 256\nextra:\n  duration_seconds: 600\n  audio_url: {}media.wav\n",
                fixture.base
            ),
        );
        std::fs::write(&path, text)?;
        git(&archive, &["add", "."])?;
        git(&archive, &["commit", "-qm", "fixture component audio"])?;
        fixture.build()?;
        let samples = 8000_u32;
        let mut wave = b"RIFF".to_vec();
        wave.extend((samples + 36).to_le_bytes());
        wave.extend(b"WAVEfmt ");
        wave.extend(16_u32.to_le_bytes());
        wave.extend(1_u16.to_le_bytes());
        wave.extend(1_u16.to_le_bytes());
        wave.extend(8000_u32.to_le_bytes());
        wave.extend(8000_u32.to_le_bytes());
        wave.extend(1_u16.to_le_bytes());
        wave.extend(8_u16.to_le_bytes());
        wave.extend(b"data");
        wave.extend(samples.to_le_bytes());
        wave.resize(wave.len() + samples as usize, 128);
        std::fs::write(fixture.out.join("media.wav"), wave)?;
        let client = browser_client().await?;
        let result = async {
                let mobile = !base_path.is_empty();
                emulate(&client, "Emulation.setDeviceMetricsOverride", json!({"width":if mobile {390}else{1280},"height":844,"deviceScaleFactor":1,"mobile":mobile})).await?;
                client.goto(&fixture.base).await?;
                wait_booted_with(&client, "document.querySelector('.search-command')").await?;
                client.execute("window.swup.navigate(arguments[0])", vec![json!(format!("{}preferences/", fixture.base))]).await?;
                wait_for(&client, "!window.swup.navigating && document.querySelector('#theme-mode')").await?;
                let report = client
                    .execute_async(
                        include_str!("../fixtures/component_lifecycle_checks.js"),
                        vec![json!(fixture.base)],
                    )
                    .await?;
                anyhow::ensure!(
                    report.get("error").is_none(),
                    "component lifecycle ({base_path}): {report}"
                );
                anyhow::ensure!(
                    report["cycles"] == 3,
                    "all navigation cycles completed: {report}"
                );
                client.goto(&format!("{}items/example/2026-09-01-story-36/", fixture.base)).await?;
                wait_for(&client, "!!document.querySelector('.native-audio.is-enhanced')").await?;
                screenshot(&client, if mobile {"podcast-player-mobile"} else {"podcast-player-desktop"}).await?;
                Ok(())
            }
            .await;
        report_failure(&client, "component-lifecycle", &result).await;
        finish(client, result).await?;
    }
    Ok(())
}
