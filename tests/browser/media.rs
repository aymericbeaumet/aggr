//! Media keeps its reserved geometry through enhancement, loading and failure.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use fantoccini::Client;
use serde_json::{Value, json};

use crate::harness::{
    Fixture, browser_client_with_load_strategy, browser_client_with_preferences, emulate, finish,
    fixture_pdf, git, report_failure, wait_booted, wait_for,
};

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn pdf_fallback_is_actionable_without_a_native_viewer_or_javascript() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client_with_preferences(
        "normal",
        json!({"plugins.always_open_pdf_externally": true}),
    )
    .await?;
    let result = async {
        for scripts_blocked in [true, false] {
            fixture.scripts_blocked.store(scripts_blocked, Ordering::Relaxed);
            client.goto(&format!("{}items/example/2026-09-01-story-41/?scripts={scripts_blocked}", fixture.base)).await?;
            wait_for(&client, "!!document.querySelector('.document-fallback a')").await?;
            anyhow::ensure!(client.execute("return navigator.pdfViewerEnabled", vec![]).await? == false, "fixture disables the real browser PDF viewer");
            let fallback = client.find(fantoccini::Locator::Css(".document-fallback a")).await?;
            anyhow::ensure!(fallback.is_displayed().await?, "native object reveals the actionable fallback when no viewer is available");
            anyhow::ensure!(fallback.attr("href").await?.as_deref() == Some(format!("{}document.pdf", fixture.base).as_str()), "fallback retains the original document URL");
            let geometry = client.execute("const frame=document.querySelector('.document-frame'),caption=document.querySelector('.document-reader figcaption'),style=getComputedStyle(caption);return {height:frame.getBoundingClientRect().height,centered:style.textAlign==='center',italic:style.fontStyle==='italic',fallback:document.querySelector('.document-fallback').textContent,booted:typeof window.Swup==='function'}", vec![]).await?;
            anyhow::ensure!(geometry["height"].as_f64().unwrap_or_default() >= 384.0 && geometry["centered"] == true && geometry["italic"] == true, "failed documents preserve viewer geometry and caption styling: {geometry}");
            if scripts_blocked { anyhow::ensure!(geometry["booted"] == false, "fallback works before application scripts load"); }
        }
        Ok(())
    }.await;
    report_failure(&client, "pdf-fallback", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn media_loading_keeps_reserved_space() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let fixture_image = std::fs::read(fixture.out.join("body.png"))?;
    let archive = fixture.directory.path().join(".aggr/data");
    for index in [36, 37] {
        let path = archive.join(format!("items/example/2026/09/2026-09-01-story-{index}.md"));
        let text = std::fs::read_to_string(&path)?;
        let text = if index == 37 {
            text.replace(
                "https://publisher.invalid/story-37",
                &format!("{}media.webm", fixture.base),
            )
        } else {
            text.replace(
                "content: feed\n",
                &format!(
                    "content: feed\nextra:\n  audio_url: {}media.wav\n",
                    fixture.base
                ),
            )
        };
        std::fs::write(path, text)?;
    }
    let items = archive.join("items/example/2026/09");
    let companions = std::fs::read_dir(&items)?.collect::<std::io::Result<Vec<_>>>()?;
    let preview = companions
        .iter()
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("2026-09-01-story-45.preview-")
        })
        .context("fixture preview")?;
    let original = companions
        .iter()
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("2026-09-01-story-45.image-")
        })
        .context("fixture archived image")?;
    for index in [35, 41] {
        let preview_name = preview
            .file_name()
            .to_string_lossy()
            .replace("story-45", &format!("story-{index}"));
        std::fs::copy(preview.path(), items.join(&preview_name))?;
        let mut metadata = format!(
            "preview:\n  file: {preview_name}\n  width: 240\n  height: 160\n  color: '#315d76'\n"
        );
        if index == 35 {
            let image_name = original
                .file_name()
                .to_string_lossy()
                .replace("story-45", "story-35");
            std::fs::copy(original.path(), items.join(&image_name))?;
            metadata.push_str(&format!("images:\n  - source: {}lead.png\n    original:\n      file: {image_name}\n      width: 640\n      height: 320\n    color: '#315d76'\n",fixture.base));
        }
        let path = items.join(format!("2026-09-01-story-{index}.md"));
        std::fs::write(
            &path,
            std::fs::read_to_string(&path)?
                .replace("content: feed\n", &format!("content: feed\n{metadata}")),
        )?;
    }
    git(&archive, &["add", "items"])?;
    git(&archive, &["commit", "-qm", "fixture native media"])?;
    fixture.build()?;
    std::fs::write(fixture.out.join("body.png"), fixture_image)?;
    std::fs::write(fixture.out.join("document.pdf"), fixture_pdf())?;
    let client = browser_client_with_load_strategy("none").await?;
    let result = media_layout_contracts(&client, &fixture).await;
    fixture.media.blocked.store(false, Ordering::Relaxed);
    fixture.media.app_blocked.store(false, Ordering::Relaxed);
    report_failure(&client, "media-layout", &result).await;
    finish(client, result).await
}

async fn media_box(client: &Client, selector: &str) -> Result<Value> {
    Ok(client
        .execute(
            r#"
      const media=document.querySelector(arguments[0]), box=media.getBoundingClientRect();
      const following=document.querySelector('[data-media-following]') || media.nextElementSibling || media.parentElement.nextElementSibling;
      return {width:box.width,height:box.height,top:box.top+scrollY,
        following:following ? following.getBoundingClientRect().top+scrollY : null,scroll:scrollY};
    "#,
            vec![json!(selector)],
        )
        .await?)
}

fn ensure_media_box_stable(before: &Value, after: &Value, label: &str) -> Result<()> {
    for field in ["width", "height", "top", "following", "scroll"] {
        if let (Some(before_value), Some(after_value)) =
            (before[field].as_f64(), after[field].as_f64())
        {
            anyhow::ensure!(
                (after_value - before_value).abs() <= 0.05,
                "{label} changed {field}: before={before}, after={after}"
            );
        }
    }
    Ok(())
}

async fn media_layout_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    emulate(client, "Network.enable", json!({})).await?;
    emulate(
        client,
        "Network.setCacheDisabled",
        json!({"cacheDisabled":true}),
    )
    .await?;
    emulate(
        client,
        "Network.setBlockedURLs",
        json!({"urls":["*://player.twitch.tv/*","*://player.vimeo.com/*","*://www.youtube-nocookie.com/*"]}),
    )
    .await?;
    for (index, markup) in [
        (
            40,
            format!(
                "<video id=\"fixture-native\" controls preload=\"metadata\" src=\"{}media.webm\"></video><p data-media-following>After the video.</p>",
                fixture.base
            ),
        ),
        (
            39,
            format!(
                "<audio id=\"fixture-native\" controls preload=\"metadata\" src=\"{}media.wav\"></audio><p data-media-following>After the audio.</p>",
                fixture.base
            ),
        ),
        (
            38,
            format!(
                "<img id=\"fixture-image\" src=\"{}body.png\" alt=\"A fallback illustration\"><p data-media-following>After the image.</p>",
                fixture.base
            ),
        ),
    ] {
        let file = fixture
            .out
            .join(format!("items/example/2026-09-01-story-{index}/index.html"));
        let html = std::fs::read_to_string(&file)?;
        std::fs::write(
            file,
            html.replace(
                "<div class=\"body e-content\">",
                &format!("<div class=\"body e-content\">{markup}"),
            ),
        )?;
    }
    let video = client.execute_async(r#"
      const done=arguments[arguments.length-1];
      const canvas=document.createElement('canvas');canvas.width=640;canvas.height=360;
      const context=canvas.getContext('2d');context.fillStyle='#315d76';context.fillRect(0,0,640,360);
      const stream=canvas.captureStream(10), recorder=new MediaRecorder(stream,{mimeType:'video/webm;codecs=vp8'}), chunks=[];
      recorder.ondataavailable=event=>chunks.push(event.data);
      recorder.onstop=()=>new Blob(chunks).arrayBuffer().then(buffer=>{
        stream.getTracks().forEach(track=>track.stop());
        done(Array.from(new Uint8Array(buffer)));
      });
      recorder.start();setTimeout(()=>recorder.stop(),250);
    "#, vec![]).await?;
    let video: Vec<u8> = serde_json::from_value(video)?;
    std::fs::write(fixture.out.join("media.webm"), video)?;
    let samples = 4000_u32;
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
    std::fs::write(
        fixture.out.join("media-player.html"),
        "<!doctype html><html><body style=\"background:#315d76;color:white\">Local video player</body></html>",
    )?;
    for width in [390, 650, 1280] {
        emulate(
            client,
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":width<600}),
        )
        .await?;
        for (index, selector, label, loaded) in [
            (41, ".document-reader", "PDF", "window.documentEvents > 0"),
            (
                35,
                ".article-lead",
                "lead image",
                "document.querySelector('.article-lead img')?.complete",
            ),
            (
                45,
                ".article-picture",
                "archived image",
                "document.querySelector('.progressive-image')?.complete",
            ),
            (
                38,
                "#fixture-image",
                "fallback image",
                "document.querySelector('#fixture-image')?.complete",
            ),
            (
                40,
                "#fixture-native",
                "native video",
                "document.querySelector('#fixture-native')?.readyState >= 1 || document.querySelector('#fixture-native')?.error",
            ),
            (
                39,
                "#fixture-native",
                "native audio",
                "document.querySelector('#fixture-native')?.readyState >= 1 || document.querySelector('#fixture-native')?.error",
            ),
            (
                37,
                ".native-video",
                "direct video",
                "document.querySelector('.native-video video')?.readyState >= 1 || document.querySelector('.native-video video')?.error",
            ),
            (
                36,
                ".native-audio",
                "podcast audio",
                "document.querySelector('.native-audio audio')?.readyState >= 1 || document.querySelector('.native-audio audio')?.error",
            ),
        ] {
            for failed in [false, true] {
                fixture.media.blocked.store(true, Ordering::Relaxed);
                fixture.media.app_blocked.store(true, Ordering::Relaxed);
                fixture.media.failed.store(failed, Ordering::Relaxed);
                client
                    .goto(&format!(
                        "{}items/example/2026-09-01-story-{index}/?media={width}-{failed}",
                        fixture.base
                    ))
                    .await?;
                wait_for(client, &format!("typeof window.swup?.navigate !== 'function' && location.search === '?media={width}-{failed}' && !!document.querySelector('{selector}') && !!document.querySelector('article.item') && getComputedStyle(document.querySelector('.top')).position === 'sticky'")).await?;
                let before_scripts = media_box(client, selector).await?;
                if matches!(label, "PDF" | "lead image" | "archived image") {
                    let placeholder=client.execute("const frame=document.querySelector('.document-frame,.article-lead,.article-picture');const background=getComputedStyle(frame,'::before').backgroundImage;const matched=background.match(/url\\([\"']?(.*?)[\"']?\\)/);window.expectedPlaceholderHref=matched?.[1];return window.expectedPlaceholderHref||null",vec![]).await?;
                    let placeholder = url::Url::parse(
                        placeholder
                            .as_str()
                            .context("computed media placeholder URL")?,
                    )?;
                    anyhow::ensure!(
                        placeholder.as_str().starts_with("data:image/png;base64,"),
                        "{label} must show an inline ThumbHash preview before scripts or image requests complete"
                    );
                    let decoded = client.execute_async("const done=arguments[arguments.length-1],image=new Image();image.onload=()=>done(image.naturalWidth>0 && image.naturalWidth<=32 && image.naturalHeight<=32);image.onerror=()=>done(false);image.src=window.expectedPlaceholderHref",vec![]).await?;
                    anyhow::ensure!(
                        decoded == true,
                        "{label} ThumbHash PNG must decode while media requests remain blocked"
                    );
                }
                client.execute(r#"
                  window.mediaEvents=0;
                  window.documentEvents=0;
                  ['load','error'].forEach(type=>document.querySelector('.document-viewer')?.addEventListener(type,()=>window.documentEvents++));
                  document.querySelectorAll('object,iframe,video,audio,img').forEach(media=>{
                    ['load','error','loadedmetadata'].forEach(type=>media.addEventListener(type,()=>window.mediaEvents++));
                  });
                "#, vec![]).await?;
                fixture.media.app_blocked.store(false, Ordering::Relaxed);
                wait_booted(client).await?;
                let mut before_media = media_box(client, selector).await?;
                ensure_media_box_stable(
                    &before_scripts,
                    &before_media,
                    &format!("{label} enhancement at {width}px"),
                )?;
                if label == "PDF" {
                    client.execute_async("const done=arguments[arguments.length-1];scrollTo(0,250);setTimeout(()=>requestAnimationFrame(()=>done(true)),350)",vec![]).await?;
                    before_media = media_box(client, selector).await?;
                }
                if matches!(label, "direct video" | "podcast audio") {
                    client.execute("const media=document.querySelector('.native-video video, .native-audio audio');media.preload='metadata';media.load()",vec![]).await?;
                }
                let completed = fixture.media.completed.load(Ordering::Relaxed);
                fixture.media.blocked.store(false, Ordering::Relaxed);
                let start = Instant::now();
                while fixture.media.completed.load(Ordering::Relaxed) == completed {
                    anyhow::ensure!(
                        start.elapsed() < Duration::from_secs(10),
                        "{label} request did not complete"
                    );
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                wait_for(client, loaded).await?;
                if !failed && label.starts_with("native") {
                    anyhow::ensure!(
                        client
                            .execute(
                                "return document.querySelector('#fixture-native').readyState >= 1",
                                vec![]
                            )
                            .await?
                            == true,
                        "{label} fixture must decode successfully"
                    );
                }
                if !failed && matches!(label, "direct video" | "podcast audio") {
                    anyhow::ensure!(
                        client.execute("return document.querySelector('.native-video video, .native-audio audio').readyState >= 1",vec![]).await?==true,
                        "{label} fixture must decode successfully"
                    );
                }
                if !failed && label.ends_with("image") {
                    anyhow::ensure!(
                        client.execute("return document.querySelector('.article-lead img, #fixture-image, .progressive-image').naturalWidth > 0", vec![]).await? == true,
                        "{label} fixture must decode successfully"
                    );
                }
                client.execute_async("const done=arguments[arguments.length-1];setTimeout(()=>requestAnimationFrame(()=>done(true)),350)", vec![]).await?;
                let after_media = media_box(client, selector).await?;
                ensure_media_box_stable(
                    &before_media,
                    &after_media,
                    &format!(
                        "{label} {} at {width}px",
                        if failed { "failure" } else { "load" }
                    ),
                )?;
                if !failed && label == "direct video" {
                    let timing = client.execute(r#"
                      const media=document.querySelector('.native-video video'),host=document.querySelector('[data-media-timing]');
                      const before=document.querySelector('.body').getBoundingClientRect().top;
                      Object.defineProperties(media,{duration:{configurable:true,value:600},currentTime:{configurable:true,value:60},paused:{configurable:true,value:false}});
                      media.playbackRate=2;
                      media.dispatchEvent(new Event('durationchange'));
                      media.dispatchEvent(new Event('playing'));
                      const playing=host.textContent,metadata=document.querySelector('.itemhead .reading-stats').textContent.trim();
                      Object.defineProperty(media,'paused',{configurable:true,value:true});
                      media.dispatchEvent(new Event('pause'));
                      return {playing,paused:host.textContent,metadata,stable:Math.abs(document.querySelector('.body').getBoundingClientRect().top-before)<=1};
                    "#,vec![]).await?;
                    anyhow::ensure!(
                        timing["playing"]
                            .as_str()
                            .is_some_and(|text| text.contains("Ends at"))
                            && !timing["paused"]
                                .as_str()
                                .unwrap_or_default()
                                .contains("Ends at")
                            && timing["metadata"] == "10 min watch"
                            && timing["stable"] == true,
                        "native video timing is accurate and does not move content: {timing}"
                    );
                }
                fixture.media.failed.store(false, Ordering::Relaxed);
            }
        }
        for (index, provider) in [(43, "twitch"), (42, "vimeo")] {
            for motion in ["auto", "off"] {
                client
                    .execute(
                        "localStorage.setItem('aggr:motion',arguments[0])",
                        vec![json!(motion)],
                    )
                    .await?;
                client
                    .goto(&format!(
                        "{}items/example/2026-09-01-story-{index}/?provider={width}-{motion}",
                        fixture.base
                    ))
                    .await?;
                wait_for(
                client,
                &format!("location.search === '?provider={width}-{motion}' && document.querySelector('.video-player')?.dataset.videoProvider === '{provider}' && document.querySelector('[data-video-embed]')?.getAttribute('role') === 'button'"),
            )
            .await?;
                client
                    .execute(
                        r#"
              document.querySelector('[data-video-embed]').dataset.videoEmbed=arguments[0];
            "#,
                        vec![json!(format!(
                            "{}media-player.html?provider={provider}&width={width}&motion={motion}",
                            fixture.base
                        ))],
                    )
                    .await?;
                let before = media_box(client, ".video-player").await?;
                fixture.media.blocked.store(true, Ordering::Relaxed);
                client
                    .execute(
                        "document.querySelector('[data-video-embed]').click()",
                        vec![],
                    )
                    .await?;
                wait_for(client, "!!document.querySelector('.video-player iframe')").await?;
                client.execute_async("const done=arguments[arguments.length-1];requestAnimationFrame(()=>requestAnimationFrame(()=>done(true)))", vec![]).await?;
                let after = media_box(client, ".video-player").await?;
                ensure_media_box_stable(
                    &before,
                    &after,
                    &format!("{provider} activation at {width}px"),
                )?;
                let pending = client.execute(r#"
              const player=document.querySelector('.video-player'), preview=player.querySelector('.video-preview'), frame=player.querySelector('iframe'), style=getComputedStyle(frame);
              return {preview:!!preview && !preview.hidden && getComputedStyle(preview).display !== 'none',covered:style.opacity === '0' || style.visibility === 'hidden' || style.display === 'none',classes:player.className,source:frame.src,opacity:style.opacity,visibility:style.visibility,display:style.display};
            "#,vec![]).await?;
                anyhow::ensure!(
                    pending["preview"] == true && pending["covered"] == true,
                    "{provider} at {width}px with motion={motion} must keep its preview visible until ready: {pending}"
                );
                fixture.media.blocked.store(false, Ordering::Relaxed);
                wait_for(
                    client,
                    "document.querySelector('.video-player').classList.contains('is-loaded')",
                )
                .await?;
                let loaded = media_box(client, ".video-player").await?;
                ensure_media_box_stable(
                    &before,
                    &loaded,
                    &format!("{provider} ready at {width}px with motion={motion}"),
                )?;
            }
        }
    }
    Ok(())
}
