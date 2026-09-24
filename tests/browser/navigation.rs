//! Navigation: archive boundaries, the speculation rules the browser prepares pages with, video
//! provider facades, and the reader's modules across base paths.

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
async fn speculation_rules_prepare_the_archive_and_nothing_outside_it() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        client.goto(&fixture.base).await?;
        wait_booted_with(&client, "document.querySelectorAll('[data-row-open]').length === 3").await?;
        // Preparing the next page is the browser's job, declared in markup rather than driven by
        // script. What matters is which links it may take at its word.
        let speculation = client.execute(r#"
          const declared = document.querySelector('script[type=speculationrules]');
          const rules = JSON.parse(declared.textContent);
          const prefetch = rules.prefetch[0], prerender = rules.prerender[0];
          const excluded = prefetch.where.and.find(clause => clause.not).not.selector_matches;
          const matches = (node, selector) => !!node && node.matches(selector);
          // Every link on this page that leaves the archive, whichever ones the fixture renders.
          const outward = [...document.querySelectorAll('a[target="_blank"], .u-bookmark-of, .config-link')];
          return {
            eagerness:[prefetch.eagerness, prerender.eagerness],
            article:matches(document.querySelector('.row [data-row-open]'), prerender.where.selector_matches),
            route:matches(document.querySelector('[data-site-navigation] a[data-route]'), prerender.where.selector_matches),
            outward:outward.length,
            kept:outward.filter(link => !link.matches(excluded)).map(link => link.className || link.href)
          };
        "#, vec![]).await?;
        anyhow::ensure!(
            speculation["eagerness"] == json!(["moderate", "moderate"])
                && speculation["article"] == true
                && speculation["route"] == true,
            "the archive's own pages are the ones worth preparing: {speculation}"
        );
        anyhow::ensure!(
            speculation["outward"].as_u64().unwrap_or(0) > 0 && speculation["kept"] == json!([]),
            "a link that leaves the archive must never be fetched before someone follows it: {speculation}"
        );
        Ok(())
    }.await;
    report_failure(&client, "speculation-rules", &result).await;
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
                    controls["count"] == 17,
                    "every setting renders a control ({base_path}): {controls}"
                );
                anyhow::ensure!(
                    controls["theme"] == controls["applied"],
                    "the form shows the value the document is using: {controls}"
                );
                client.goto(&format!("{}items/example/2026-09-01-story-36/", fixture.base)).await?;
                wait_for(&client, "!!document.querySelector('.native-audio.is-enhanced')").await?;
                screenshot(&client, if mobile {"podcast-player-mobile"} else {"podcast-player-desktop"}).await?;
                Ok(())
            }
            .await;
        report_failure(&client, "reader-modules", &result).await;
        finish(client, result).await?;
    }
    Ok(())
}
