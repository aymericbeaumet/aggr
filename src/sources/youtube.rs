use url::Url;

use crate::{content::ExtractedArticle, http};

pub fn thumbnail(url: &Url) -> Option<String> {
    let id = video_id(url)?;
    Some(format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"))
}

pub fn poster_candidates(url: &Url) -> Vec<crate::media::Candidate> {
    let Some(id) = video_id(url) else {
        return Vec::new();
    };
    ["maxresdefault.jpg", "hqdefault.jpg"]
        .into_iter()
        .filter_map(|name| Url::parse(&format!("https://i.ytimg.com/vi/{id}/{name}")).ok())
        .map(|url| crate::media::Candidate { url, alt: None })
        .collect()
}

pub(crate) fn video_id(url: &Url) -> Option<String> {
    if !is_video_url(url) {
        return None;
    }
    let id = if url.path() == "/watch" {
        url.query_pairs()
            .find(|(key, _)| key == "v")?
            .1
            .into_owned()
    } else if matches!(url.host_str(), Some("youtu.be" | "www.youtu.be")) {
        url.path_segments()?.next()?.to_string()
    } else {
        url.path_segments()?.nth(1)?.to_string()
    };
    (!id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')))
    .then_some(id)
}

/// Only follow caption tracks advertised by the public player page. Missing, restricted, or
/// rate-limited captions leave the original feed description intact.
pub async fn extract(
    page: &str,
    video: &Url,
    description: Option<&str>,
    client: &http::Client,
) -> ExtractedArticle {
    let mut html = description.unwrap_or_default().to_string();
    if let Some(track) = caption_track(page) {
        match fetch_transcript(&track, video, client).await {
            Ok(Some(transcript)) => html.push_str(&transcript),
            Ok(None) => {}
            Err(error) => log::debug!("YouTube captions unavailable for {video}: {error:#}"),
        }
    }
    ExtractedArticle {
        html,
        image: thumbnail(video),
    }
}

async fn fetch_transcript(
    track: &Url,
    video: &Url,
    client: &http::Client,
) -> anyhow::Result<Option<String>> {
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        client.get(http::Request {
            url: track,
            headers: &[],
            etag: None,
            last_modified: None,
        }),
    )
    .await??;
    Ok(match response {
        http::Response::Ok(body) => transcript_html(&body.bytes, video),
        http::Response::NotModified => None,
    })
}

fn caption_track(page: &str) -> Option<Url> {
    let player = page
        .match_indices("ytInitialPlayerResponse")
        .find_map(|(start, _)| {
            let tail = &page[start + "ytInitialPlayerResponse".len()..];
            let json = &tail[tail.find('{')?..];
            serde_json::Deserializer::from_str(json)
                .into_iter::<serde_json::Value>()
                .next()?
                .ok()
        })?;
    let captions = player.pointer("/captions/playerCaptionsTracklistRenderer")?;
    let tracks = captions.get("captionTracks")?.as_array()?;
    let preferred = captions
        .pointer("/audioTracks/0/defaultCaptionTrackIndex")
        .and_then(|value| value.as_u64())
        .and_then(|index| usize::try_from(index).ok());
    let mut ordered = tracks.iter().enumerate().collect::<Vec<_>>();
    ordered.sort_by_key(|(index, track)| {
        (
            Some(*index) != preferred,
            track.get("kind").and_then(|value| value.as_str()) == Some("asr"),
            *index,
        )
    });
    ordered.into_iter().find_map(|(_, track)| {
        let mut url = Url::parse(track.get("baseUrl")?.as_str()?).ok()?;
        if url.scheme() != "https"
            || !matches!(url.host_str(), Some("www.youtube.com" | "youtube.com"))
            || url.path() != "/api/timedtext"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
        {
            return None;
        }
        let query = url
            .query_pairs()
            .filter(|(key, _)| key != "fmt")
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        url.query_pairs_mut()
            .clear()
            .extend_pairs(query)
            .append_pair("fmt", "json3");
        Some(url)
    })
}

fn transcript_html(bytes: &[u8], video: &Url) -> Option<String> {
    let data: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let mut paragraphs: Vec<(u64, String)> = Vec::new();
    for event in data.get("events")?.as_array()?.iter().take(10_000) {
        let Some(start) = event.get("tStartMs").and_then(|value| value.as_u64()) else {
            continue;
        };
        let Some(segments) = event.get("segs").and_then(|value| value.as_array()) else {
            continue;
        };
        let text = segments
            .iter()
            .filter_map(|segment| segment.get("utf8").and_then(|value| value.as_str()))
            .collect::<String>();
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            continue;
        }
        if let Some((first, paragraph)) = paragraphs.last_mut()
            && start >= *first
            && start - *first < 30_000
            && paragraph.len() + text.len() < 600
        {
            paragraph.push(' ');
            paragraph.push_str(&text);
        } else {
            paragraphs.push((start, text));
        }
    }
    if paragraphs.is_empty() {
        return None;
    }
    let mut html = String::from("<section><h2>Transcript</h2>");
    for (start, text) in paragraphs {
        let seconds = start / 1_000;
        let id = video_id(video)?;
        let href = format!("https://www.youtube.com/watch?v={id}&t={seconds}");
        let timestamp = if seconds >= 3_600 {
            format!(
                "{}:{:02}:{:02}",
                seconds / 3_600,
                seconds / 60 % 60,
                seconds % 60
            )
        } else {
            format!("{}:{:02}", seconds / 60, seconds % 60)
        };
        html.push_str(&format!(
            "<p><a href=\"{}\">{timestamp}</a> {}</p>",
            escape_html(&href),
            escape_html(&text)
        ));
        if html.len() >= 200_000 {
            break;
        }
    }
    html.push_str("</section>");
    Some(html)
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn is_short_url(url: &Url) -> bool {
    is_youtube(url) && has_video_path(url, "shorts")
}

pub fn is_video_url(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    if matches!(url.host_str(), Some("youtu.be" | "www.youtu.be")) {
        return url
            .path_segments()
            .is_some_and(|mut parts| parts.next().is_some_and(|id| !id.is_empty()));
    }
    if matches!(
        url.host_str(),
        Some("youtube-nocookie.com" | "www.youtube-nocookie.com")
    ) {
        return has_video_path(url, "embed");
    }
    is_youtube(url)
        && ((url.path() == "/watch"
            && url
                .query_pairs()
                .any(|(key, value)| key == "v" && !value.is_empty()))
            || ["shorts", "live", "embed", "v"]
                .iter()
                .any(|prefix| has_video_path(url, prefix)))
}

fn is_youtube(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url
            .host_str()
            .is_some_and(|host| host == "youtube.com" || host.ends_with(".youtube.com"))
}

fn has_video_path(url: &Url, prefix: &str) -> bool {
    url.path_segments().is_some_and(|mut parts| {
        parts.next() == Some(prefix) && parts.next().is_some_and(|id| !id.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn captions_fetch_json_and_fail_gracefully_without_text() {
        crate::http::install_crypto_provider();
        let server = httpmock::MockServer::start_async().await;
        let track = server.mock_async(|when, then| {
            when.path("/captions");
            then.status(200).json_body(serde_json::json!({"events":[{"tStartMs":9000,"segs":[{"utf8":"Caption text"}]}]}));
        }).await;
        let empty = server
            .mock_async(|when, then| {
                when.path("/empty");
                then.status(200).body("");
            })
            .await;
        let denied = server
            .mock_async(|when, then| {
                when.path("/denied");
                then.status(403);
            })
            .await;
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let video = Url::parse("https://youtube.com/watch?v=video").unwrap();
        assert!(
            fetch_transcript(
                &Url::parse(&server.url("/captions")).unwrap(),
                &video,
                &client
            )
            .await
            .unwrap()
            .unwrap()
            .contains(">0:09</a> Caption text")
        );
        assert!(
            fetch_transcript(&Url::parse(&server.url("/empty")).unwrap(), &video, &client)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            fetch_transcript(
                &Url::parse(&server.url("/denied")).unwrap(),
                &video,
                &client
            )
            .await
            .is_err()
        );
        let fallback = extract(
            "<html>Consent or unavailable</html>",
            &video,
            Some("<p>Description</p>"),
            &client,
        )
        .await;
        assert_eq!(fallback.html, "<p>Description</p>");
        assert!(fallback.image.is_some());
        track.assert_async().await;
        empty.assert_async().await;
        denied.assert_async().await;
    }

    #[test]
    fn thumbnails_use_video_ids_and_ignore_untrusted_urls() {
        assert_eq!(
            thumbnail(&Url::parse("https://youtu.be/abc_DEF-123?t=9").unwrap()).as_deref(),
            Some("https://i.ytimg.com/vi/abc_DEF-123/hqdefault.jpg")
        );
        assert!(thumbnail(&Url::parse("https://youtube.com/watch?v=../x").unwrap()).is_none());
        let posters = poster_candidates(&Url::parse("https://youtu.be/abc_DEF-123?t=9").unwrap());
        assert_eq!(posters.len(), 2);
        assert_eq!(
            posters[0].url.as_str(),
            "https://i.ytimg.com/vi/abc_DEF-123/maxresdefault.jpg"
        );
        assert_eq!(
            posters[1].url.as_str(),
            "https://i.ytimg.com/vi/abc_DEF-123/hqdefault.jpg"
        );
        assert!(
            poster_candidates(&Url::parse("https://youtube.com/watch?v=../x").unwrap()).is_empty()
        );
    }

    #[test]
    fn public_caption_track_prefers_original_manual_language() {
        let page = r#"<script>var ytInitialPlayerResponse = {"captions":{"playerCaptionsTracklistRenderer":{"captionTracks":[{"baseUrl":"https://www.youtube.com/api/timedtext?v=abc&lang=en","languageCode":"en","kind":"asr"},{"baseUrl":"https://www.youtube.com/api/timedtext?v=abc&lang=fr","languageCode":"fr"}],"audioTracks":[{"defaultCaptionTrackIndex":1}]}}};</script>"#;
        let track = caption_track(page).unwrap();
        assert!(track.as_str().contains("lang=fr"));
        assert!(track.as_str().contains("fmt=json3"));
        assert!(
            caption_track(&page.replace(
                "https://www.youtube.com/api/timedtext",
                "https://evil.test/steal"
            ))
            .is_none()
        );
    }

    #[test]
    fn timed_captions_are_readable_safe_and_link_to_the_video() {
        let json = br#"{"events":[{"tStartMs":1200,"segs":[{"utf8":"Hello "},{"utf8":"<world> & friends"}]},{"tStartMs":65400,"segs":[{"utf8":"Second\nline"}]},{"tStartMs":66000},{"tStartMs":-1,"segs":[{"utf8":"invalid"}]}]}"#;
        let html = transcript_html(
            json,
            &Url::parse("https://youtu.be/abc_DEF-123?t=4").unwrap(),
        )
        .unwrap();
        assert!(html.contains("<h2>Transcript</h2>"));
        assert!(html.contains("t=1"));
        assert!(html.contains(">0:01</a>"));
        assert!(html.contains(">1:05</a>"));
        assert!(html.contains("Hello &lt;world&gt; &amp; friends"));
        assert!(html.contains("Second line"));
        assert!(!html.contains("invalid"));
        let markdown = crate::content::to_markdown(&html, None);
        let rendered = crate::content::render_markdown(&markdown);
        assert!(rendered.contains("t=65"));
        assert!(rendered.contains("&lt;world&gt;"));
        assert!(!rendered.contains("<world>"));
        assert!(
            transcript_html(
                br#"{"events":[]}"#,
                &Url::parse("https://youtu.be/abc").unwrap()
            )
            .is_none()
        );
    }

    #[test]
    fn recognizes_youtube_video_urls_without_matching_other_sites() {
        for link in [
            "https://www.youtube.com/watch?v=video",
            "https://m.youtube.com/watch?v=video&t=42",
            "https://youtu.be/video",
            "https://www.youtube.com/shorts/video",
            "https://www.youtube.com/live/video",
            "https://www.youtube-nocookie.com/embed/video",
        ] {
            assert!(is_video_url(&Url::parse(link).unwrap()), "{link}");
        }
        for link in [
            "https://www.youtube.com/@channel",
            "https://www.youtube.com/feeds/videos.xml?channel_id=channel",
            "https://www.youtube.com/watch?list=playlist",
            "https://youtu.be/",
            "https://youtube.com.example.com/watch?v=video",
            "https://example.com/watch?v=video",
            "ftp://www.youtube.com/watch?v=video",
        ] {
            assert!(!is_video_url(&Url::parse(link).unwrap()), "{link}");
        }
    }

    #[test]
    fn excludes_shorts_only_on_youtube() {
        for link in [
            "https://www.youtube.com/shorts/video",
            "https://youtube.com/shorts/video?feature=share",
            "https://m.youtube.com/shorts/video/",
        ] {
            assert!(is_short_url(&Url::parse(link).unwrap()), "{link}");
        }
        for link in [
            "https://www.youtube.com/watch?v=video",
            "https://www.youtube.com/shorts",
            "https://www.youtube.com/shorts/",
            "https://example.com/shorts/video",
            "https://youtube.com.example.com/shorts/video",
            "https://example.com/?url=https://youtube.com/shorts/video",
        ] {
            assert!(!is_short_url(&Url::parse(link).unwrap()), "{link}");
        }
    }
}
