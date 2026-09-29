//! Navigation: archive boundaries, pages swapped in place and fetched ahead, video provider
//! facades, and the reader's modules across base paths.

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
async fn article_boundary_keys_return_to_feed() -> Result<()> {
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
        // Both ends of the archive: stepping past either one lands on the feed instead of
        // stopping dead on an article that has no neighbour in that direction.
        for (index, direction, input) in [(45, "previous", "k"), (1, "next", "j")] {
            client
                .goto(&format!(
                    "{}items/example/2026-09-01-story-{index:02}/",
                    fixture.base
                ))
                .await?;
            wait_booted_with(&client, "document.querySelector('article.item .body p')").await?;
            let missing = client
                .execute(
                    "return !document.querySelector('article.item').dataset[arguments[0]+'Url']",
                    vec![json!(direction)],
                )
                .await?;
            anyhow::ensure!(
                missing == true,
                "{index} must be the real {direction} archive boundary"
            );
            client
                .execute("document.activeElement?.blur()", vec![])
                .await?;
            key(&client, input).await?;
            wait_booted_with(
                &client,
                &format!(
                    "location.href==={} && document.body.dataset.kind==='river'",
                    json!(fixture.base)
                ),
            )
            .await?;
        }
        Ok(())
    }
    .await;
    report_failure(&client, "article-boundary-navigation", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn video_provider_facades() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::new()?;
    let client = browser_client().await?;
    let result = catch_panics(video_facade_contracts(&client, &fixture)).await;
    report_failure(&client, "video-facades", &result).await;
    finish(client, result).await
}

async fn video_facade_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    phone_session(client).await?;
    client.goto(&fixture.base).await?;
    wait_booted_with(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3",
    )
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
            "document.querySelector('.video-player[data-video-bound=\"true\"] [data-video-embed]')",
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
            "document.querySelector('.video-player[data-video-bound=\"true\"] [data-video-embed]')",
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
async fn archive_pages_swap_in_place_and_nothing_outside_the_archive_is_fetched() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        client.goto(&fixture.base).await?;
        wait_booted_with(&client, "document.querySelectorAll('[data-row-open]').length === 3").await?;
        // Pages are fetched ahead by the reader itself, so the browser's own speculation rules
        // would only prepare documents no click will load.
        anyhow::ensure!(
            client.execute("return !document.querySelector('script[type=speculationrules]')", vec![]).await? == true,
            "no speculation rules compete with the reader's own fetching"
        );
        // Once the page has settled, the tabs and the rows the reader lingers on are fetched
        // ahead; a link that leaves the archive never is, whichever ones the fixture renders.
        wait_for(&client, r#"(() => {
          const fetched = new Set(performance.getEntriesByType('resource').filter(entry => entry.initiatorType === 'fetch').map(entry => entry.name));
          return [...document.querySelectorAll('.mobile-tabs a[data-route], .rows .row [data-row-open]')].every(link => fetched.has(link.href) || link.href.split('#')[0] === location.href.split('#')[0]);
        })()"#).await?;
        let outward = client.execute(r#"
          const fetched = new Set(performance.getEntriesByType('resource').map(entry => entry.name));
          const outward = [...document.querySelectorAll('a[target="_blank"], .u-bookmark-of, .config-link')];
          return {count: outward.length, fetched: outward.filter(link => fetched.has(link.href)).map(link => link.href)};
        "#, vec![]).await?;
        anyhow::ensure!(
            outward["count"].as_u64().unwrap_or(0) > 0 && outward["fetched"] == json!([]),
            "a link that leaves the archive must never be fetched before someone follows it: {outward}"
        );
        // Following a row swaps the article in: the same document, a new address, its own title
        // and kind, and one history entry for it.
        let before = client.execute(r#"
          window.__aggrDocument = 'kept';
          return {history: history.length, href: document.querySelector('.row [data-row-open]').href, title: document.querySelector('.row [data-row-open]').textContent.trim()};
        "#, vec![]).await?;
        client.find(Locator::Css(".row [data-row-open]")).await?.click().await?;
        wait_booted_with(&client, "document.body.dataset.kind === 'item'").await?;
        let arrived = client.execute(r#"
          return {kept: window.__aggrDocument, href: location.href, title: document.querySelector('.itemhead h1').textContent.trim(),
            documentTitle: document.title, top: scrollY, history: history.length, kind: window.AGGR.kind,
            tab: document.querySelector('.mobile-tabs [aria-current]')?.dataset.route ?? null,
            base: new URL(window.AGGR.base, location.href).href};
        "#, vec![]).await?;
        anyhow::ensure!(
            arrived["kept"] == "kept" && arrived["href"] == before["href"] && arrived["title"] == before["title"]
                && arrived["documentTitle"].as_str().unwrap_or("").contains(before["title"].as_str().unwrap_or("?"))
                && arrived["top"] == 0 && arrived["kind"] == "item" && arrived["tab"] == Value::Null
                && arrived["history"].as_u64() == before["history"].as_u64().map(|length| length + 1)
                && arrived["base"] == fixture.base.as_str(),
            "an archive link swaps the page in place: {arrived} after {before}"
        );
        // Back is instant too: the feed returns with its place and its cursor, still in place.
        client.back().await?;
        wait_booted_with(&client, "document.body.dataset.kind === 'river' && !!document.querySelector('.row.is-selected')").await?;
        let returned = client.execute(r#"
          return {kept: window.__aggrDocument, selected: document.querySelector('.row.is-selected [data-row-open]').href,
            tab: document.querySelector('.mobile-tabs [aria-current]')?.dataset.route};
        "#, vec![]).await?;
        anyhow::ensure!(
            returned["kept"] == "kept" && returned["tab"] == "" && returned["selected"] == before["href"],
            "Back swaps the feed back in with its cursor: {returned}"
        );
        // A page arriving while the last one still glides is held where it belongs until it is
        // still; asking for its top in the meantime is the reader's call, and the hold lets go.
        let held = client.execute_async(r#"
          const done = arguments[arguments.length - 1], root = document.documentElement;
          const frame = () => new Promise(resolve => requestAnimationFrame(() => resolve()));
          (async () => {
            root.style.paddingBottom = '200vh';
            scrollTo(0, 300); await frame(); await frame();
            document.querySelector('.row [data-row-open]').click();
            while (document.body.dataset.kind !== 'item') await frame();
            // A fling carrying the article as Back is pressed.
            const fling = () => { if (document.body.dataset.kind === 'item') { scrollBy(0, 4); requestAnimationFrame(fling); } };
            fling(); await frame(); await frame();
            history.back();
            while (document.body.dataset.kind !== 'river') await new Promise(resolve => setTimeout(resolve));
            const arrived = {held: root.style.overflow === 'hidden', at: scrollY};
            document.querySelector('.brand').click();
            for (let i = 0; i < 20; i++) await frame();
            done({...arrived, top: scrollY, overflow: root.style.overflow});
            root.style.removeProperty('padding-bottom');
          })();
        "#, vec![]).await?;
        anyhow::ensure!(
            held["held"] == true && held["at"] == 300 && held["top"] == 0 && held["overflow"] == "",
            "the title takes a held page to its top and ends the hold: {held}"
        );
        Ok(())
    }.await;
    report_failure(&client, "in-place-navigation", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn reader_modules_survive_every_base_path() -> Result<()> {
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
                // Preferences survives an ordinary navigation: its controls are server-rendered
                // and the reader's saved values are applied before paint.
                client.goto(&format!("{}preferences/", fixture.base)).await?;
                wait_booted_with(&client, "document.querySelector('#theme')").await?;
                let controls: serde_json::Value = client
                    .execute(
                        "return {count: document.querySelectorAll('[data-preference]').length, \
                         theme: document.querySelector('#theme').value, \
                         applied: document.documentElement.dataset.theme};",
                        vec![],
                    )
                    .await?;
                anyhow::ensure!(
                    controls["count"] == 18,
                    "every setting renders a control ({base_path}): {controls}"
                );
                anyhow::ensure!(
                    controls["theme"] == controls["applied"],
                    "the form shows the value the document is using: {controls}"
                );
                client.goto(&format!("{}items/example/2026-09-01-story-36/", fixture.base)).await?;
                wait_for(&client, "!!document.querySelector('.native-audio.is-enhanced')").await?;
                screenshot(&client, if mobile {"podcast-player-mobile"} else {"podcast-player-desktop"}).await?;
                // The player names what is playing, shows the archived length before anything
                // loads, and marks each jump with its length inside a circling arrow.
                let player = client.execute(r#"
                  const card = document.querySelector('.native-audio');
                  return {episode: card.querySelector('.audio-episode')?.textContent.trim(),
                    title: document.querySelector('.itemhead h1').textContent.trim(),
                    heading: card.querySelector('.audio-heading').innerText,
                    duration: card.querySelector('[data-audio-duration]').textContent,
                    skips: [...card.querySelectorAll('[data-audio-skip]')].map(button => button.querySelector('svg') ? button.querySelector('span')?.textContent : null)};
                "#, vec![]).await?;
                anyhow::ensure!(
                    player["episode"] == player["title"]
                        && !player["heading"].as_str().unwrap_or("").contains("Listen")
                        && !player["heading"].as_str().unwrap_or("").contains("Ready when you are")
                        && player["duration"] == "10:00"
                        && player["skips"] == json!(["30", "15", "15", "30"]),
                    "the player is titled by its episode and shows its length ({base_path}): {player}"
                );
                // Idle recordings show remaining duration without claiming a finish time.
                let volume = client.execute(r#"
                  const card = document.querySelector('.native-audio'), volume = card.querySelector('[data-audio-volume]');
                  volume.value = '0.5';
                  volume.dispatchEvent(new Event('input', {bubbles: true}));
                  return {estimate: card.querySelector('[data-audio-ends-at]').textContent, text: card.innerText};
                "#, vec![]).await?;
                anyhow::ensure!(
                    volume["estimate"] == "10:00 remaining" && !volume["text"].as_str().unwrap_or("").contains("Ends at"),
                    "an idle player shows duration, not a finish time ({base_path}): {volume}"
                );
                // A player reached in place is enhanced like one that was loaded.
                client.find(Locator::Css(".brand")).await?.click().await?;
                wait_booted_with(&client, "document.body.dataset.kind === 'river' && !document.querySelector('.native-audio')").await?;
                client.back().await?;
                wait_for(&client, "document.body.dataset.kind === 'item' && !!document.querySelector('.native-audio.is-enhanced')").await?;
                Ok(())
            }
            .await;
        report_failure(&client, "reader-modules", &result).await;
        finish(client, result).await?;
    }
    Ok(())
}
